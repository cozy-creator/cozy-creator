//! Local research probe: real cryptography, simulated proxy authorization map.
//! Synthetic data only. This is not a production protocol or storage format.
use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
    Aes256Gcm, Nonce,
};
#[cfg(not(feature = "api480"))]
use recrypt::api::{
    CryptoOps, Ed25519Ops, EncryptedValue, KeyGenOps, PrivateKey, PublicKey, Recrypt,
    SigningKeypair, TransformKey,
};
#[cfg(feature = "api480")]
use recrypt::api_480::{
    CryptoOps, Ed25519Ops, EncryptedValue, KeyGenOps, PrivateKey, PublicKey, Recrypt480 as Recrypt,
    SigningKeypair, TransformKey,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

fn component_bytes(value: &EncryptedValue) -> usize {
    let public_bytes = |key: &PublicKey| key.bytes_x_y().0.len() + key.bytes_x_y().1.len();
    match value {
        EncryptedValue::EncryptedOnceValue {
            ephemeral_public_key,
            encrypted_message,
            auth_hash,
            public_signing_key,
            signature,
        } => {
            public_bytes(ephemeral_public_key)
                + encrypted_message.bytes().len()
                + auth_hash.bytes().len()
                + public_signing_key.bytes().len()
                + signature.bytes().len()
        }
        EncryptedValue::TransformedValue {
            ephemeral_public_key,
            encrypted_message,
            auth_hash,
            public_signing_key,
            signature,
            transform_blocks,
        } => {
            public_bytes(ephemeral_public_key)
                + encrypted_message.bytes().len()
                + auth_hash.bytes().len()
                + public_signing_key.bytes().len()
                + signature.bytes().len()
                + transform_blocks
                    .to_vec()
                    .iter()
                    .map(|b| {
                        public_bytes(b.public_key())
                            + public_bytes(b.random_transform_public_key())
                            + b.encrypted_temp_key().bytes().len()
                            + b.encrypted_random_transform_temp_key().bytes().len()
                    })
                    .sum::<usize>()
        }
    }
}

fn recover_key(
    recrypt: &impl CryptoOps,
    capsule: EncryptedValue,
    recipient: &PrivateKey,
) -> Option<[u8; 32]> {
    recrypt
        .decrypt(capsule, recipient)
        .ok()
        .map(|plaintext| *recrypt.derive_symmetric_key(&plaintext).bytes())
}

fn open(key: &[u8; 32], nonce: &[u8], ciphertext: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
    Aes256Gcm::new_from_slice(key)
        .unwrap()
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .ok()
}

// This is deliberately a local authorization simulation, not a server integration.
fn proxy_request(
    recrypt: &impl CryptoOps,
    grants: &HashMap<&str, TransformKey>,
    user: &str,
    capsule: &EncryptedValue,
    signer: &SigningKeypair,
) -> Option<EncryptedValue> {
    grants.get(user).map(|key| {
        recrypt
            .transform(capsule.clone(), key.clone(), signer)
            .unwrap()
    })
}

fn main() {
    println!(
        "api={}",
        if cfg!(feature = "api480") {
            "api_480"
        } else {
            "api_256"
        }
    );
    let recrypt = Recrypt::new();
    let signer = recrypt.generate_ed25519_key_pair();
    let (group_sk, group_pk) = recrypt.generate_key_pair().unwrap();
    let (alice_sk, alice_pk) = recrypt.generate_key_pair().unwrap();
    let (bob_sk, bob_pk) = recrypt.generate_key_pair().unwrap();
    let mut grants = HashMap::<&str, TransformKey>::new();

    // One fresh independent DEK per object/version. No permanent project DEK.
    let object_secret = recrypt.gen_plaintext();
    let dek = recrypt.derive_symmetric_key(&object_secret);
    let capsule = recrypt.encrypt(&object_secret, &group_pk, &signer).unwrap();
    let payload: Vec<u8> = (0..4 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let aad = b"synthetic-project:object-1:version-1";
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = Aes256Gcm::new_from_slice(dek.bytes())
        .unwrap()
        .encrypt(&nonce, Payload { msg: &payload, aad })
        .unwrap();
    let unchanged_ciphertext = ciphertext.clone();
    let initial_capsule = capsule.clone();
    assert!(recover_key(&recrypt, capsule.clone(), &bob_sk).is_none());

    grants.insert(
        "alice",
        recrypt
            .generate_transform_key(&group_sk, &alice_pk, &signer)
            .unwrap(),
    );
    let alice_capsule = proxy_request(&recrypt, &grants, "alice", &capsule, &signer).unwrap();
    let alice_dek = recover_key(&recrypt, alice_capsule, &alice_sk).unwrap();
    assert_eq!(open(&alice_dek, &nonce, &ciphertext, aad).unwrap(), payload);
    assert!(proxy_request(&recrypt, &grants, "bob", &capsule, &signer).is_none());
    println!("alice_allowed_bob_no_grant=true");

    // The map models AuthKit/Hub authorization. The transforms and AEAD are real.
    let bob_grant = recrypt
        .generate_transform_key(&group_sk, &bob_pk, &signer)
        .unwrap();
    let dishonestly_retained_grant = bob_grant.clone();
    grants.insert("bob", bob_grant);
    let cached_bob_capsule = proxy_request(&recrypt, &grants, "bob", &capsule, &signer).unwrap();
    let cached_bob_dek = recover_key(&recrypt, cached_bob_capsule.clone(), &bob_sk).unwrap();
    assert_eq!(
        open(&cached_bob_dek, &nonce, &ciphertext, aad).unwrap(),
        payload
    );
    println!("bob_grant_allows_decryption=true");
    println!("plaintext_bytes={}", payload.len());
    println!("ciphertext_bytes={}", ciphertext.len());
    println!("capsule_component_bytes={}", component_bytes(&capsule));
    println!(
        "transformed_capsule_component_bytes={}",
        component_bytes(&cached_bob_capsule)
    );
    println!("component_sizes_exclude_serialization_framing=true");

    grants.remove("bob");
    assert!(proxy_request(&recrypt, &grants, "bob", &capsule, &signer).is_none());
    println!("simulated_honest_proxy_denies_bob_after_revoke=true");
    let from_cached_capsule = recover_key(&recrypt, cached_bob_capsule, &bob_sk).unwrap();
    assert_eq!(
        open(&from_cached_capsule, &nonce, &ciphertext, aad).unwrap(),
        payload
    );
    assert_eq!(
        open(&cached_bob_dek, &nonce, &ciphertext, aad).unwrap(),
        payload
    );
    println!("cached_capsule_and_cached_dek_still_decrypt=true");

    // New object under the same group public key, but a fresh DEK/capsule.
    let fresh_secret = recrypt.gen_plaintext();
    let fresh_dek = recrypt.derive_symmetric_key(&fresh_secret);
    let fresh_capsule = recrypt.encrypt(&fresh_secret, &group_pk, &signer).unwrap();
    assert!(proxy_request(&recrypt, &grants, "bob", &fresh_capsule, &signer).is_none());
    let fresh_nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let fresh_aad = b"synthetic-project:object-2:version-1";
    let fresh_ciphertext = Aes256Gcm::new_from_slice(fresh_dek.bytes())
        .unwrap()
        .encrypt(
            &fresh_nonce,
            Payload {
                msg: &payload,
                aad: fresh_aad,
            },
        )
        .unwrap();
    assert!(open(&cached_bob_dek, &fresh_nonce, &fresh_ciphertext, fresh_aad).is_none());
    assert!(recover_key(&recrypt, fresh_capsule.clone(), &bob_sk).is_none());
    println!("old_dek_does_not_open_new_object=true");

    // A proxy retaining Bob's old transform key can keep serving fresh capsules.
    let illicit_capsule = recrypt
        .transform(fresh_capsule, dishonestly_retained_grant.clone(), &signer)
        .unwrap();
    let illicit_dek = recover_key(&recrypt, illicit_capsule, &bob_sk).unwrap();
    assert_eq!(
        open(&illicit_dek, &fresh_nonce, &fresh_ciphertext, fresh_aad).unwrap(),
        payload
    );
    println!("retained_proxy_grant_plus_bob_key_opens_new_same_group_object=true");

    // Fresh project keypair ends the old transform-key capability for future objects.
    let (next_group_sk, next_group_pk) = recrypt.generate_key_pair().unwrap();
    let next_capsule = recrypt
        .encrypt(&fresh_secret, &next_group_pk, &signer)
        .unwrap();
    let wrong_epoch = recrypt.transform(next_capsule.clone(), dishonestly_retained_grant, &signer);
    assert!(wrong_epoch
        .ok()
        .and_then(|c| recover_key(&recrypt, c, &bob_sk))
        .is_none());
    let next_alice_grant = recrypt
        .generate_transform_key(&next_group_sk, &alice_pk, &signer)
        .unwrap();
    let next_alice_capsule = recrypt
        .transform(next_capsule, next_alice_grant, &signer)
        .unwrap();
    assert_eq!(
        recover_key(&recrypt, next_alice_capsule, &alice_sk).unwrap(),
        *fresh_dek.bytes()
    );
    println!("new_group_epoch_blocks_old_grant_and_allows_new_alice_grant=true");

    // Check the library's published augmentation primitive separately, not a DCP implementation.
    let (server_share_sk, server_share_pk) = recrypt.generate_key_pair().unwrap();
    let augmented_pk = group_pk.augment(&server_share_pk).unwrap();
    let augmented_capsule = recrypt
        .encrypt(&object_secret, &augmented_pk, &signer)
        .unwrap();
    assert!(recover_key(&recrypt, augmented_capsule.clone(), &group_sk).is_none());
    assert!(recover_key(&recrypt, augmented_capsule.clone(), &server_share_sk).is_none());
    let augmented_grant = recrypt
        .generate_transform_key(&group_sk, &alice_pk, &signer)
        .unwrap()
        .augment(&server_share_sk)
        .unwrap();
    let augmented_alice = recrypt
        .transform(augmented_capsule, augmented_grant, &signer)
        .unwrap();
    assert_eq!(
        recover_key(&recrypt, augmented_alice, &alice_sk).unwrap(),
        *dek.bytes()
    );
    println!("augmentation_admin_share_and_server_share_cannot_decrypt_individually=true");
    println!("augmented_transform_allows_member=true");

    assert_eq!(capsule, initial_capsule);
    assert_eq!(ciphertext, unchanged_ciphertext);
    println!("original_capsule_and_payload_unchanged_across_grant_revoke=true");
    println!("ciphertext_sha256={:x}", Sha256::digest(&ciphertext));
    println!("result=PASS");
}
