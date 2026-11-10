//! Real Argon2id/AES-GCM envelopes, filesystem persistence and clock deadlines; no mocks or test-only KDF.
//! Run as an e2e fixture, not a replacement for the installed CLI/mount proof.
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use project_vault::{
    Credential, Keybag, ProjectSecret, RecoverySecret, SecretString, SessionPolicy, SessionTimer,
};

fn pass(value: &str) -> SecretString {
    SecretString::from(value.to_owned())
}
fn same(a: &ProjectSecret, b: &ProjectSecret) -> bool {
    a.with_bytes(|x| b.with_bytes(|y| x == y))
}
fn require(condition: bool, label: &str) {
    if !condition {
        panic!("vault fixture: {label}")
    }
    println!("PASS {label}");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("output directory required")?,
    );
    fs::create_dir_all(&dir)?;
    let made = Keybag::create("synthetic-project", pass("synthetic initial passphrase"))?;
    let key_path = dir.join("project.vault.keys.json");
    made.keybag.save(&key_path)?;
    let mut bag = Keybag::load(&key_path)?;
    let first = bag.unlock(Credential::Passphrase(pass("synthetic initial passphrase")))?;
    let recovered = bag.unlock(Credential::Recovery(pass(made.recovery_secret.expose())))?;
    require(
        same(&first, &made.secret) && same(&first, &recovered),
        "passphrase and recovery open the same project secret",
    );
    require(
        first.with_bytes(|s| s.len() == 43 && s.iter().all(u8::is_ascii)),
        "helper secret is printable full-entropy input",
    );
    require(
        bag.unlock(Credential::Passphrase(pass("wrong password")))
            .is_err(),
        "wrong passphrase refuses",
    );

    drop(first);
    drop(recovered);
    let mut tampered = serde_json::to_value(&bag)?;
    let bytes = STANDARD.decode(
        tampered["passphrase_envelope"]["ciphertext"]
            .as_str()
            .ok_or("ciphertext absent")?,
    )?;
    let mut bytes = bytes;
    *bytes.last_mut().ok_or("ciphertext empty")? ^= 1;
    tampered["passphrase_envelope"]["ciphertext"] = STANDARD.encode(bytes).into();
    let tampered: Keybag = serde_json::from_value(tampered)?;
    require(
        tampered
            .unlock(Credential::Passphrase(pass("synthetic initial passphrase")))
            .is_err(),
        "authenticated envelope corruption refuses",
    );
    let mut swapped = serde_json::to_value(&bag)?;
    swapped["passphrase_envelope"] = swapped["recovery_envelope"].clone();
    let swapped: Keybag = serde_json::from_value(swapped)?;
    require(
        swapped
            .unlock(Credential::Passphrase(pass(made.recovery_secret.expose())))
            .is_err(),
        "recovery ciphertext cannot substitute for the passphrase slot",
    );

    let mut source = serde_json::to_value(&bag)?;
    source["project_id"] = "other-project".into();
    let swapped: Keybag = serde_json::from_value(source)?;
    require(
        swapped
            .unlock(Credential::Passphrase(pass("synthetic initial passphrase")))
            .is_err(),
        "encrypted payload binds project identity",
    );
    let mut source = serde_json::to_value(&bag)?;
    source["generation"] = 2.into();
    let swapped: Keybag = serde_json::from_value(source)?;
    require(
        swapped
            .unlock(Credential::Passphrase(pass("synthetic initial passphrase")))
            .is_err(),
        "encrypted payload binds generation",
    );

    let recovered = bag.unlock(Credential::Recovery(pass(made.recovery_secret.expose())))?;
    bag.change_passphrase(&recovered, pass("synthetic replacement passphrase"))?;
    bag.save(&key_path)?;
    bag = Keybag::load(&key_path)?;
    require(
        bag.unlock(Credential::Passphrase(pass("synthetic initial passphrase")))
            .is_err(),
        "old passphrase cannot open replaced live slot",
    );
    require(
        same(
            &recovered,
            &bag.unlock(Credential::Passphrase(pass(
                "synthetic replacement passphrase",
            )))?,
        ),
        "passphrase change preserves the project secret",
    );
    let new_recovery = bag.replace_recovery(&recovered)?;
    require(
        bag.unlock(Credential::Recovery(pass(made.recovery_secret.expose())))
            .is_err(),
        "old recovery cannot open replaced live slot",
    );
    require(
        same(
            &recovered,
            &bag.unlock(Credential::Recovery(pass(new_recovery.expose())))?,
        ),
        "new recovery preserves the project secret",
    );

    let raw = serde_json::to_string(&bag)?;
    require(
        !raw.contains(new_recovery.expose())
            && recovered.with_bytes(|s| !raw.as_bytes().windows(s.len()).any(|w| w == s)),
        "serialized keybag contains neither recovery nor filesystem secret",
    );
    require(
        !format!("{recovered:?}")
            .contains(&recovered.with_bytes(|s| String::from_utf8_lossy(s).into_owned()))
            && !format!("{new_recovery:?}").contains(new_recovery.expose()),
        "debug output redacts unlocked secrets",
    );
    let presented = new_recovery.expose().to_uppercase().replace('-', " \n");
    require(
        RecoverySecret::parse(pass(&presented))?.expose() == new_recovery.expose(),
        "recovery accepts presentation whitespace and case",
    );
    let mut typo = new_recovery.expose().to_owned();
    typo.pop();
    typo.push('z');
    require(
        RecoverySecret::parse(pass(&typo)).is_err(),
        "recovery transcription checksum refuses errors",
    );

    let target = bag.new_generation(pass("synthetic rotation passphrase"))?;
    require(
        target.keybag.info().generation == 2
            && target.keybag.info().key_id != bag.info().key_id
            && !same(&target.secret, &recovered),
        "full rekey creates a fresh key generation",
    );
    require(
        target
            .keybag
            .unlock(Credential::Recovery(pass(target.recovery_secret.expose())))?
            .generation()
            == 2,
        "new generation has independent working recovery",
    );
    require(
        bag.change_passphrase(&target.secret, pass("synthetic mismatch"))
            .is_err(),
        "slot update refuses another generation",
    );

    // Received KDF settings are validated against fixed resource bounds before any derivation.
    let mut excessive = serde_json::to_value(&bag)?;
    excessive["passphrase_envelope"]["memory_kib"] = (1u32 << 30).into();
    let excessive: Keybag = serde_json::from_value(excessive)?;
    require(
        excessive
            .unlock(Credential::Passphrase(pass(
                "synthetic replacement passphrase",
            )))
            .is_err(),
        "untrusted excessive KDF work refuses",
    );
    let huge = dir.join("oversize.json");
    fs::write(&huge, vec![b' '; 65 * 1024])?;
    require(
        Keybag::load(&huge).is_err(),
        "oversize keybag refuses before parsing",
    );

    let mut idle = SessionTimer::new(SessionPolicy {
        idle_seconds: Some(1),
        maximum_seconds: None,
    });
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1150) {
        let _ = idle.expired();
        std::thread::sleep(Duration::from_millis(30));
    }
    require(
        idle.expired() && !idle.activity() && idle.expired(),
        "status polling cannot extend or revive idle expiry",
    );
    let mut maximum = SessionTimer::new(SessionPolicy {
        idle_seconds: Some(10),
        maximum_seconds: Some(1),
    });
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1150) {
        let _ = maximum.activity();
        std::thread::sleep(Duration::from_millis(30));
    }
    require(
        maximum.expired() && !maximum.activity(),
        "absolute expiry wins over active interaction",
    );
    fs::write(dir.join("receipt.json"), b"{\"keybag\":\"pass\",\"rotation\":\"pass\",\"bounds\":\"pass\",\"timeouts\":\"pass\",\"synthetic_data_only\":true}\n")?;
    println!(
        "project-vault native engine: passed (actual cryptography; CLI and mount proof remain separate)"
    );
    Ok(())
}
