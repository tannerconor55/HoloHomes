use ed25519_dalek::{Signer, SigningKey};
use holochain::prelude::*;
use holochain::sweettest::*;
use rentals::review::{CreateReviewInput, ListingReviews};
use rentals_integrity::BookingDecision;
use serde::{Deserialize, Serialize};

mod common;
use common::*;

/// The payload `agent_linking/create_external_link` takes, shaped exactly as ProofPoll's
/// Tauri backend sends it (`external_signature` as a plain `Vec<u8>`).
#[derive(Serialize, Deserialize, Debug)]
struct CreateExternalLinkInput {
    external_agent: AgentPubKey,
    external_signature: Vec<u8>,
}

/// What Flowsta Vault's `POST /link-identity` does: sort the two raw 39-byte agent keys,
/// concatenate them, and sign the 78 raw bytes with the Vault's Ed25519 key.
fn vault_sign(vault_key: &SigningKey, vault_agent: &AgentPubKey, app_agent: &AgentPubKey) -> Vec<u8> {
    let mut keys = [
        vault_agent.get_raw_39().to_vec(),
        app_agent.get_raw_39().to_vec(),
    ];
    keys.sort();
    vault_key.sign(&keys.concat()).to_bytes().to_vec()
}

#[tokio::test(flavor = "multi_thread")]
async fn flowsta_vault_sign_in_is_verified_and_shown_on_reviews() {
    let (conductors, cells) = setup(2).await;
    let (host, guest) = (conductors.get(0).unwrap(), conductors.get(1).unwrap());
    let guest_agent = cells[1].agent_pubkey().clone();
    let guest_linking = cells[1].zome("agent_linking");
    let host_linking = cells[0].zome("agent_linking");

    let vault_key = SigningKey::from_bytes(&[7u8; 32]);
    let vault_agent = AgentPubKey::from_raw_32(vault_key.verifying_key().to_bytes().to_vec());

    // A signature over anything but the sorted key pair is refused.
    assert_error_contains(
        guest
            .call_fallible::<_, ActionHash>(
                &guest_linking,
                "create_external_link",
                CreateExternalLinkInput {
                    external_agent: vault_agent.clone(),
                    external_signature: vault_key.sign(b"not the key pair").to_bytes().to_vec(),
                },
            )
            .await,
        "does not verify",
    );

    let attestation_hash: ActionHash = guest
        .call(
            &guest_linking,
            "create_external_link",
            CreateExternalLinkInput {
                external_agent: vault_agent.clone(),
                external_signature: vault_sign(&vault_key, &vault_agent, &guest_agent),
            },
        )
        .await;
    await_consistency(&cells).await.unwrap();

    // Any peer can resolve the link in both directions.
    let linked: Vec<AgentPubKey> = host
        .call(&host_linking, "get_linked_agents", guest_agent.clone())
        .await;
    assert_eq!(linked, vec![vault_agent.clone()]);
    let linked: Vec<AgentPubKey> = host
        .call(&host_linking, "get_linked_agents", vault_agent.clone())
        .await;
    assert_eq!(linked, vec![guest_agent.clone()]);

    // A review from the signed-in guest carries the attestation.
    let listing = create_listing(host, &cells[0], "Belem house", 38.6970, -9.2060).await;
    await_consistency(&cells).await.unwrap();
    let stay = request_booking(guest, &cells[1], &listing, -4, -2).await;
    await_consistency(&cells).await.unwrap();
    let _: Record = host
        .call(
            &cells[0].zome("rentals"),
            "respond_to_booking",
            response_input(&stay, BookingDecision::Accepted),
        )
        .await;
    await_consistency(&cells).await.unwrap();
    let review: Record = guest
        .call(
            &cells[1].zome("rentals"),
            "create_review",
            CreateReviewInput {
                booking_request_hash: stay,
                rating: 4,
                text: "Great views".into(),
            },
        )
        .await;
    let review: rentals_integrity::Review = review.entry().to_app_option().unwrap().unwrap();
    assert_eq!(review.identity_link_hash, Some(attestation_hash.clone()));
    await_consistency(&cells).await.unwrap();

    let reviews: ListingReviews = host
        .call(&cells[0].zome("rentals"), "get_reviews_for_listing", listing.clone())
        .await;
    assert_eq!(reviews.reviews[0].flowsta_identity, Some(vault_agent.clone()));
    assert_eq!(reviews.summary.verified_review_count, 1);

    // Revoking the attestation removes the verified badge.
    let _: ActionHash = guest
        .call(&guest_linking, "revoke_link", attestation_hash)
        .await;
    await_consistency(&cells).await.unwrap();
    let linked: Vec<AgentPubKey> = host
        .call(&host_linking, "get_linked_agents", guest_agent)
        .await;
    assert!(linked.is_empty());
    let reviews: ListingReviews = host
        .call(&cells[0].zome("rentals"), "get_reviews_for_listing", listing)
        .await;
    assert_eq!(reviews.reviews[0].flowsta_identity, None);
    assert_eq!(reviews.summary.verified_review_count, 0);
}
