//! Throwaway OpenMLS evaluation; labels/envelopes are synthetic, not a Cozy schema.
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::{OpenMlsProvider, random::OpenMlsRand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use tls_codec::Deserialize as _;

const SUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
const OBJECT_AAD: &[u8] = b"xs160 evaluation synthetic encrypted object";
const STATE_AAD: &[u8] = b"xs160 evaluation synthetic device state";
const ARCHIVE_AAD: &[u8] = b"xs160 evaluation synthetic archived key";

struct Actor {
    provider: OpenMlsRustCrypto,
    signer: SignatureKeyPair,
    credential: CredentialWithKey,
}

impl Actor {
    fn new(name: &str) -> Self {
        let provider = OpenMlsRustCrypto::default();
        let signer = SignatureKeyPair::new(SUITE.signature_algorithm()).unwrap();
        signer.store(provider.storage()).unwrap();
        let credential = CredentialWithKey {
            credential: BasicCredential::new(name.as_bytes().to_vec()).into(),
            signature_key: signer.to_public_vec().into(),
        };
        Self {
            provider,
            signer,
            credential,
        }
    }

    fn key_package(&self) -> KeyPackageBundle {
        KeyPackage::builder()
            .build(SUITE, &self.provider, &self.signer, self.credential.clone())
            .unwrap()
    }
}

#[derive(Serialize, Deserialize)]
struct Sealed {
    nonce: [u8; 12],
    ciphertext: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct SyntheticObjectKey {
    object: String,
    key: Vec<u8>,
}

fn seal(provider: &OpenMlsRustCrypto, key: &[u8], aad: &[u8], bytes: &[u8]) -> Sealed {
    let nonce = provider.rand().random_array::<12>().unwrap();
    let cipher = Aes256Gcm::new_from_slice(key).unwrap();
    Sealed {
        nonce,
        ciphertext: cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: bytes, aad })
            .unwrap(),
    }
}

fn open(key: &[u8], aad: &[u8], sealed: &Sealed) -> Result<Vec<u8>, String> {
    Aes256Gcm::new_from_slice(key)
        .unwrap()
        .decrypt(
            Nonce::from_slice(&sealed.nonce),
            Payload {
                msg: &sealed.ciphertext,
                aad,
            },
        )
        .map_err(|e| format!("{e:?}"))
}

fn store_bytes(provider: &OpenMlsRustCrypto) -> Vec<u8> {
    let mut rows: Vec<_> = provider
        .storage()
        .values
        .read()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    serde_json::to_vec(&rows).unwrap()
}

fn restore_provider(bytes: &[u8]) -> OpenMlsRustCrypto {
    let provider = OpenMlsRustCrypto::default();
    let rows: Vec<(Vec<u8>, Vec<u8>)> = serde_json::from_slice(bytes).unwrap();
    *provider.storage().values.write().unwrap() = rows.into_iter().collect();
    provider
}

fn protocol(bytes: &[u8]) -> ProtocolMessage {
    MlsMessageIn::tls_deserialize_exact(bytes)
        .unwrap()
        .try_into_protocol_message()
        .unwrap()
}

fn join(actor: &Actor, welcome: &MlsMessageOut) -> Result<MlsGroup, String> {
    let message = MlsMessageIn::tls_deserialize_exact(welcome.to_bytes().unwrap()).unwrap();
    let MlsMessageBodyIn::Welcome(welcome) = message.extract() else {
        return Err("expected Welcome".into());
    };
    let config = MlsGroupJoinConfig::builder()
        .use_ratchet_tree_extension(true)
        .set_past_epoch_deletion_policy(PastEpochDeletionPolicy::MaxEpochs(0))
        .build();
    StagedWelcome::new_from_welcome(&actor.provider, &config, welcome, None)
        .map_err(|e| format!("{e:?}"))?
        .into_group(&actor.provider)
        .map_err(|e| format!("{e:?}"))
}

fn send(actor: &Actor, group: &mut MlsGroup, bytes: &[u8]) -> Vec<u8> {
    group
        .create_message(&actor.provider, &actor.signer, bytes)
        .unwrap()
        .to_bytes()
        .unwrap()
}

fn receive(actor: &Actor, group: &mut MlsGroup, bytes: &[u8]) -> Result<(String, Vec<u8>), String> {
    let processed = group
        .process_message(&actor.provider, protocol(bytes))
        .map_err(|e| format!("{e:?}"))?;
    let identity = String::from_utf8(processed.credential().serialized_content().to_vec()).unwrap();
    match processed.into_content() {
        ProcessedMessageContent::ApplicationMessage(message) => {
            Ok((identity, message.into_bytes()))
        }
        _ => Err("unexpected non-application message".into()),
    }
}

fn merge(actor: &Actor, group: &mut MlsGroup, commit: &[u8]) -> Result<String, String> {
    let processed = group
        .process_message(&actor.provider, protocol(commit))
        .map_err(|e| format!("{e:?}"))?;
    let identity = String::from_utf8(processed.credential().serialized_content().to_vec()).unwrap();
    match processed.into_content() {
        ProcessedMessageContent::StagedCommitMessage(staged) => {
            group
                .merge_staged_commit(&actor.provider, *staged)
                .map_err(|e| format!("{e:?}"))?;
            Ok(identity)
        }
        _ => Err("unexpected non-commit message".into()),
    }
}

fn members(group: &MlsGroup) -> Vec<String> {
    group
        .members()
        .map(|m| String::from_utf8(m.credential.serialized_content().to_vec()).unwrap())
        .collect()
}

fn same_epoch(left: &MlsGroup, right: &MlsGroup) -> bool {
    left.epoch() == right.epoch()
        && left.epoch_authenticator().as_slice() == right.epoch_authenticator().as_slice()
}

fn main() {
    let mut results: Vec<Value> = Vec::new();
    let alice = Actor::new("Alice");
    let bob = Actor::new("Bob");
    let carol = Actor::new("Carol");
    let dave = Actor::new("Dave");
    let bob_package = bob.key_package();
    let carol_package = carol.key_package();
    let dave_package = dave.key_package();
    let create = MlsGroupCreateConfig::builder()
        .ciphersuite(SUITE)
        .use_ratchet_tree_extension(true)
        .set_past_epoch_deletion_policy(PastEpochDeletionPolicy::MaxEpochs(0))
        .build();
    let group_id = GroupId::from_slice(b"xs160-evaluation-only");
    let mut ag = MlsGroup::new_with_group_id(
        &alice.provider,
        &alice.signer,
        &create,
        group_id.clone(),
        alice.credential.clone(),
    )
    .unwrap();
    assert_eq!(ag.epoch().as_u64(), 0);

    let (_, bob_welcome, _) = ag
        .add_members(
            &alice.provider,
            &alice.signer,
            &[bob_package.key_package().clone()],
        )
        .unwrap();
    drop(bob_package);
    let stranger = Actor::new("Stranger");
    let stranger_join_error = join(&stranger, &bob_welcome).err().unwrap();
    ag.merge_pending_commit(&alice.provider).unwrap();
    let mut bg = join(&bob, &bob_welcome).unwrap();
    assert!(same_epoch(&ag, &bg));
    assert_eq!(members(&ag), ["Alice", "Bob"]);
    results.push(json!({"case":"authenticated_add_and_welcome", "epoch":ag.epoch().as_u64(), "members":members(&bg), "epoch_authenticators_equal":true, "stranger_without_private_keypackage_rejected":stranger_join_error}));

    let object_plaintext = b"Synthetic immutable project history: revision 1, object payload.";
    let object_key = alice.provider.rand().random_array::<32>().unwrap();
    let object_ciphertext = seal(&alice.provider, &object_key, OBJECT_AAD, object_plaintext);
    fs::write(
        "synthetic-encrypted-object.json",
        serde_json::to_vec(&object_ciphertext).unwrap(),
    )
    .unwrap();
    let key_envelope = SyntheticObjectKey {
        object: "synthetic-revision-1".into(),
        key: object_key.to_vec(),
    };
    let key_bytes = serde_json::to_vec(&key_envelope).unwrap();
    let historical_message = send(&alice, &mut ag, &key_bytes);
    assert_eq!(
        MlsMessageIn::tls_deserialize_exact(&historical_message)
            .unwrap()
            .wire_format(),
        WireFormat::PrivateMessage
    );
    let (sender, received_key_bytes) = receive(&bob, &mut bg, &historical_message).unwrap();
    assert_eq!(sender, "Alice");
    let bob_key: SyntheticObjectKey = serde_json::from_slice(&received_key_bytes).unwrap();
    assert_eq!(
        open(&bob_key.key, OBJECT_AAD, &object_ciphertext).unwrap(),
        object_plaintext
    );
    let replay_error = receive(&bob, &mut bg, &historical_message).unwrap_err();
    let mut tampered = send(&alice, &mut ag, b"tamper rejection test");
    let middle = tampered.len() / 2;
    tampered[middle] ^= 0x80;
    let tamper_error = receive(&bob, &mut bg, &tampered).unwrap_err();
    results.push(json!({"case":"application_content_key", "epoch":1, "authenticated_sender":sender, "mls_ciphertext_bytes":historical_message.len(), "wire_format":"PrivateMessage", "object_plaintext_bytes":object_plaintext.len(), "bob_decrypted_object":true, "replay_rejected":replay_error, "tamper_rejected":tamper_error}));

    let delayed_epoch1 = send(&alice, &mut ag, b"unread epoch 1 message");
    let bob_public = bob.signer.to_public_vec();
    let bob_credential = bob.credential.clone();
    let before_rows = bob.provider.storage().values.read().unwrap().len();
    let local_kek = alice.provider.rand().random_array::<32>().unwrap();
    let device_state = seal(
        &alice.provider,
        &local_kek,
        STATE_AAD,
        &store_bytes(&bob.provider),
    );
    fs::write(
        "encrypted-local-state.json",
        serde_json::to_vec(&device_state).unwrap(),
    )
    .unwrap();
    drop(bg);
    drop(bob);
    let on_disk: Sealed =
        serde_json::from_slice(&fs::read("encrypted-local-state.json").unwrap()).unwrap();
    let restored = restore_provider(&open(&local_kek, STATE_AAD, &on_disk).unwrap());
    let signer =
        SignatureKeyPair::read(restored.storage(), &bob_public, SUITE.signature_algorithm())
            .unwrap();
    let bob = Actor {
        provider: restored,
        signer,
        credential: bob_credential,
    };
    let mut bg = MlsGroup::load(bob.provider.storage(), &group_id)
        .unwrap()
        .unwrap();
    assert!(same_epoch(&ag, &bg));
    let after_reload = send(
        &alice,
        &mut ag,
        b"message after encrypted local-state reload",
    );
    assert_eq!(
        receive(&bob, &mut bg, &after_reload).unwrap().1,
        b"message after encrypted local-state reload"
    );
    results.push(json!({"case":"encrypted_local_state_reload", "epoch":bg.epoch().as_u64(), "storage_rows":before_rows, "encrypted_snapshot_bytes":fs::metadata("encrypted-local-state.json").unwrap().len(), "original_provider_group_signer_dropped":true, "signature_key_loaded_from_store":true, "epoch_authenticators_equal":true, "new_message_decrypted":true, "external_state_wrapping_key_retained":true}));

    // A counterfactual Bob retaining two prior epochs can receive delayed data.
    let retained_provider = restore_provider(&store_bytes(&bob.provider));
    let retained_signer = SignatureKeyPair::read(
        retained_provider.storage(),
        &bob_public,
        SUITE.signature_algorithm(),
    )
    .unwrap();
    let retained_bob = Actor {
        provider: retained_provider,
        signer: retained_signer,
        credential: bob.credential.clone(),
    };
    let mut retained_bg = MlsGroup::load(retained_bob.provider.storage(), &group_id)
        .unwrap()
        .unwrap();
    retained_bg
        .set_past_epoch_deletion_policy(
            &retained_bob.provider,
            PastEpochDeletionPolicy::MaxEpochs(2),
        )
        .unwrap();
    let (carol_commit, carol_welcome, _) = ag
        .add_members(
            &alice.provider,
            &alice.signer,
            &[carol_package.key_package().clone()],
        )
        .unwrap();
    drop(carol_package);
    merge(&bob, &mut bg, &carol_commit.to_bytes().unwrap()).unwrap();
    ag.merge_pending_commit(&alice.provider).unwrap();
    let mut cg = join(&carol, &carol_welcome).unwrap();
    assert!(same_epoch(&ag, &cg));
    let carol_history_error = receive(&carol, &mut cg, &historical_message).unwrap_err();
    let bob_delayed_error = receive(&bob, &mut bg, &delayed_epoch1).unwrap_err();
    merge(
        &retained_bob,
        &mut retained_bg,
        &carol_commit.to_bytes().unwrap(),
    )
    .unwrap();
    assert_eq!(
        receive(&retained_bob, &mut retained_bg, &delayed_epoch1)
            .unwrap()
            .1,
        b"unread epoch 1 message"
    );
    let explicit_reshare = send(&alice, &mut ag, &key_bytes);
    let reshared_key: SyntheticObjectKey =
        serde_json::from_slice(&receive(&carol, &mut cg, &explicit_reshare).unwrap().1).unwrap();
    assert_eq!(
        open(&reshared_key.key, OBJECT_AAD, &object_ciphertext).unwrap(),
        object_plaintext
    );
    results.push(json!({"case":"new_member_history", "epoch":2, "members":members(&cg), "carol_prior_mls_message_rejected":carol_history_error, "bob_unread_prior_epoch_rejected_with_max_epochs_0":bob_delayed_error, "counterfactual_bob_unread_prior_epoch_decrypted_with_max_epochs_2":true, "carol_history_readable_after_explicit_content_key_reshare":true, "automatic_history_grant":false}));

    // Two genuine, different membership commits are created from the same epoch.
    let carol_index = bg
        .members()
        .find(|m| m.credential.serialized_content() == b"Carol")
        .unwrap()
        .index;
    let (add_dave, dave_welcome, _) = ag
        .add_members(
            &alice.provider,
            &alice.signer,
            &[dave_package.key_package().clone()],
        )
        .unwrap();
    drop(dave_package);
    let (remove_carol_losing, _, _) = bg
        .remove_members(&bob.provider, &bob.signer, &[carol_index])
        .unwrap();
    assert_eq!(ag.epoch().as_u64(), 2);
    assert_eq!(bg.epoch().as_u64(), 2);
    assert!(ag.pending_commit().is_some() && bg.pending_commit().is_some());
    let pending_before = bg.pending_commit().is_some();
    // Simulate a sequencer accepting Alice's add-Dave commit first.
    let accepted_bytes = add_dave.to_bytes().unwrap();
    merge(&bob, &mut bg, &accepted_bytes).unwrap();
    merge(&carol, &mut cg, &accepted_bytes).unwrap();
    ag.merge_pending_commit(&alice.provider).unwrap();
    let mut dg = join(&dave, &dave_welcome).unwrap();
    assert!(same_epoch(&ag, &bg) && same_epoch(&ag, &cg) && same_epoch(&ag, &dg));
    let stale_commit_error =
        merge(&alice, &mut ag, &remove_carol_losing.to_bytes().unwrap()).unwrap_err();
    assert!(bg.pending_commit().is_none());
    let (remove_carol_retry, _, _) = bg
        .remove_members(&bob.provider, &bob.signer, &[carol_index])
        .unwrap();
    let retry_bytes = remove_carol_retry.to_bytes().unwrap();
    merge(&alice, &mut ag, &retry_bytes).unwrap();
    merge(&dave, &mut dg, &retry_bytes).unwrap();
    merge(&carol, &mut cg, &retry_bytes).unwrap();
    bg.merge_pending_commit(&bob.provider).unwrap();
    assert!(same_epoch(&ag, &bg) && same_epoch(&ag, &dg));
    assert!(!cg.is_active());
    results.push(json!({"case":"concurrent_membership_commits", "base_epoch":2, "alice_operation":"add Dave", "bob_operation":"remove Carol", "bob_had_pending_commit_when_winner_received":pending_before, "accepted_alice_commit_epoch":3, "losing_commit_rejected":stale_commit_error, "losing_pending_commit_cleared_when_winner_merged":true, "bob_recreated_remove_commit_at_epoch":3, "after_retry_epoch":4, "members":members(&ag), "carol_active":cg.is_active(), "survivor_epoch_authenticators_equal":true, "no_application_acl_prevented_bob_removing_carol":true}));

    // Keep a lagging epoch-4 Bob to prove withholding his removal commit does not help.
    let lagging_provider = restore_provider(&store_bytes(&bob.provider));
    let lagging_signer = SignatureKeyPair::read(
        lagging_provider.storage(),
        &bob_public,
        SUITE.signature_algorithm(),
    )
    .unwrap();
    let lagging_bob = Actor {
        provider: lagging_provider,
        signer: lagging_signer,
        credential: bob.credential.clone(),
    };
    let mut lagging_bg = MlsGroup::load(lagging_bob.provider.storage(), &group_id)
        .unwrap()
        .unwrap();
    let bob_index = ag
        .members()
        .find(|m| m.credential.serialized_content() == b"Bob")
        .unwrap()
        .index;
    let (remove_bob, _, _) = ag
        .remove_members(&alice.provider, &alice.signer, &[bob_index])
        .unwrap();
    let remove_bytes = remove_bob.to_bytes().unwrap();
    merge(&dave, &mut dg, &remove_bytes).unwrap();
    merge(&bob, &mut bg, &remove_bytes).unwrap();
    ag.merge_pending_commit(&alice.provider).unwrap();
    let new_content_key = alice.provider.rand().random_array::<32>().unwrap();
    let after_removal = send(&alice, &mut ag, &new_content_key);
    assert_eq!(
        receive(&dave, &mut dg, &after_removal).unwrap().1,
        new_content_key
    );
    let removed_error = receive(&bob, &mut bg, &after_removal).unwrap_err();
    let lagging_error = receive(&lagging_bob, &mut lagging_bg, &after_removal).unwrap_err();
    assert_eq!(
        open(&bob_key.key, OBJECT_AAD, &object_ciphertext).unwrap(),
        object_plaintext
    );
    results.push(json!({"case":"remove_member", "new_epoch":ag.epoch().as_u64(), "members":members(&ag), "removed_bob_active":bg.is_active(), "bob_after_removal_rejected":removed_error, "bob_without_removal_commit_epoch":lagging_bg.epoch().as_u64(), "bob_without_removal_commit_rejected":lagging_error, "retained_dave_decrypted_new_content_key":true, "bob_previously_obtained_object_key_still_decrypts_old_object":true}));

    // A deliberately separate archive experiment. The random recovery wrapping
    // key models an already available external backup, never an MLS identity.
    let recovery_key = alice.provider.rand().random_array::<32>().unwrap();
    let archive = seal(&alice.provider, &recovery_key, ARCHIVE_AAD, &key_bytes);
    fs::write(
        "synthetic-archival-envelope.json",
        serde_json::to_vec(&archive).unwrap(),
    )
    .unwrap();
    let fresh_device = OpenMlsRustCrypto::default();
    assert!(
        MlsGroup::load(fresh_device.storage(), &group_id)
            .unwrap()
            .is_none()
    );
    let archive_on_disk: Sealed =
        serde_json::from_slice(&fs::read("synthetic-archival-envelope.json").unwrap()).unwrap();
    let recovered_key: SyntheticObjectKey =
        serde_json::from_slice(&open(&recovery_key, ARCHIVE_AAD, &archive_on_disk).unwrap())
            .unwrap();
    let recovered_object: Sealed =
        serde_json::from_slice(&fs::read("synthetic-encrypted-object.json").unwrap()).unwrap();
    assert_eq!(
        open(&recovered_key.key, OBJECT_AAD, &recovered_object).unwrap(),
        object_plaintext
    );
    let wrong_recovery_key = fresh_device.rand().random_array::<32>().unwrap();
    let missing_key_error = open(&wrong_recovery_key, ARCHIVE_AAD, &archive_on_disk).unwrap_err();
    results.push(json!({"case":"archive_and_device_loss_boundary", "fresh_device_can_load_mls_group":false, "separately_wrapped_content_key_restores_old_object_with_external_recovery_key":true, "wrong_recovery_key_rejected":missing_key_error, "mls_group_reconstructed_by_archive":false, "recovery_key_not_derived_from_account_or_mls_identity":true, "durable_backup_protocol_implemented":false}));

    let output = json!({"run_date":"2026-11-09", "openmls":"0.9.2", "provider":"openmls_rust_crypto 0.6.0", "basic_credential":"0.6.0", "memory_storage":"0.6.0", "tls_codec":"0.5.0", "synthetic_archive_aead":"aes-gcm 0.10.3 AES-256-GCM", "ciphersuite":format!("{SUITE:?}"), "past_epoch_deletion_policy":"MaxEpochs(0)", "all_assertions_passed":true, "results":results});
    let rendered = serde_json::to_string_pretty(&output).unwrap();
    fs::write("results.json", &rendered).unwrap();
    println!("{rendered}");
}
