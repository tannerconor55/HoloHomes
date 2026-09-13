//! The pairwise "is same person" attestation behind Flowsta Vault sign-in.
//!
//! Ported from the `agent_linking` zome in WeAreFlowsta/flowsta-identity-dna v1.4
//! (Apache-2.0, Holochain 0.6) to hdi 0.8, keeping the entry shape and signed payload
//! unchanged so attestations made by Flowsta Vault verify here.
//!
//! One difference matters: the Vault's `POST /link-identity` endpoint signs the *raw* 78-byte
//! sorted key pair, so signatures are checked with `verify_signature_raw`. `verify_signature`
//! would MessagePack-encode the payload first and reject every Vault signature.
//!
//! This crate only holds shared types and checks; it defines no zome externs, so both
//! integrity zomes can compile it in.

use hdi::prelude::*;

/// Two agent keys attesting they belong to the same person.
///
/// Both agents sign `sorted_agent_pair_bytes`; either one may commit the entry.
#[hdk_entry_helper]
#[derive(Clone, PartialEq)]
pub struct IsSamePersonEntry {
    /// The key whose raw 39 bytes sort first.
    pub agent_a: AgentPubKey,
    pub signature_a: Signature,
    /// The key whose raw 39 bytes sort second.
    pub agent_b: AgentPubKey,
    pub signature_b: Signature,
    /// Unix seconds. Redundant with the action timestamp; kept so the entry matches
    /// Flowsta's `IsSamePersonEntry` field for field.
    pub created_at: i64,
}

impl IsSamePersonEntry {
    pub fn involves(&self, agent: &AgentPubKey) -> bool {
        &self.agent_a == agent || &self.agent_b == agent
    }

    /// The agent paired with `agent`, if `agent` is part of this attestation.
    pub fn other_agent(&self, agent: &AgentPubKey) -> Option<&AgentPubKey> {
        if &self.agent_a == agent {
            Some(&self.agent_b)
        } else if &self.agent_b == agent {
            Some(&self.agent_a)
        } else {
            None
        }
    }
}

/// The 78-byte payload both agents sign: their raw 39-byte keys, sorted and concatenated.
/// Byte-for-byte identical to Flowsta Vault's `build_sorted_agent_pair_payload`.
pub fn sorted_agent_pair_bytes(agent_a: &AgentPubKey, agent_b: &AgentPubKey) -> Vec<u8> {
    let (first, second) = if agent_a.get_raw_39() <= agent_b.get_raw_39() {
        (agent_a, agent_b)
    } else {
        (agent_b, agent_a)
    };
    [first.get_raw_39(), second.get_raw_39()].concat()
}

/// Checks an attestation on its own terms: two distinct agents in canonical order, each of
/// whom signed the sorted pair. `Ok(Err(reason))` means the attestation is invalid.
pub fn check_attestation(attestation: &IsSamePersonEntry) -> ExternResult<Result<(), String>> {
    let (a, b) = (attestation.agent_a.get_raw_39(), attestation.agent_b.get_raw_39());
    if a == b {
        return Ok(Err("An identity attestation must link two different agents".into()));
    }
    if a > b {
        return Ok(Err("agent_a must sort before agent_b".into()));
    }
    let payload = sorted_agent_pair_bytes(&attestation.agent_a, &attestation.agent_b);
    if !verify_signature_raw(
        attestation.agent_a.clone(),
        attestation.signature_a.clone(),
        payload.clone(),
    )? {
        return Ok(Err("signature_a does not verify against agent_a".into()));
    }
    if !verify_signature_raw(
        attestation.agent_b.clone(),
        attestation.signature_b.clone(),
        payload,
    )? {
        return Ok(Err("signature_b does not verify against agent_b".into()));
    }
    Ok(Ok(()))
}
