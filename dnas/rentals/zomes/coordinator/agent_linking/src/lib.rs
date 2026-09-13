//! Flowsta-compatible agent linking: turns a Flowsta Vault sign-in into an on-DHT attestation.
//!
//! The zome name and the `create_external_link` / `get_linked_agents` / `revoke_link`
//! functions match what `@flowsta/holochain` and ProofPoll call.
//!
//! Sign-in flow:
//! 1. The UI calls `linkFlowstaIdentity({ appName, clientId, localAgentPubKey })`.
//! 2. The Vault asks the user to approve, then returns `vaultAgentPubKey` and `vaultSignature`.
//! 3. The UI calls `create_external_link` with those two values, and this zome co-signs and
//!    commits the attestation.

use agent_linking_integrity::*;
use hdk::prelude::*;

#[hdk_extern]
pub fn init() -> ExternResult<InitCallbackResult> {
    Ok(InitCallbackResult::Pass)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CreateExternalLinkInput {
    /// The Vault's agent key (`vaultAgentPubKey`, decoded from its `uhCAk…` form).
    pub external_agent: AgentPubKey,
    /// The Vault's 64-byte signature (`vaultSignature`, base64-decoded). `serde_bytes`
    /// accepts both a MessagePack binary (JS `Uint8Array`) and an array of numbers (Rust `Vec<u8>`).
    #[serde(with = "serde_bytes")]
    pub external_signature: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AgentPair {
    pub agent_a: AgentPubKey,
    pub agent_b: AgentPubKey,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct IdentityLink {
    pub action_hash: ActionHash,
    /// The agent on the other side of the attestation, e.g. the Flowsta Vault key.
    pub linked_agent: AgentPubKey,
    pub attestation: IsSamePersonEntry,
}

fn guest_error(message: impl Into<String>) -> WasmError {
    wasm_error!(WasmErrorInner::Guest(message.into()))
}

/// Verifies the external agent's signature, adds this agent's own signature, and commits the
/// attestation with a lookup link from each agent. Returns the attestation's action hash.
#[hdk_extern]
pub fn create_external_link(input: CreateExternalLinkInput) -> ExternResult<ActionHash> {
    let me = agent_info()?.agent_initial_pubkey;
    if me == input.external_agent {
        return Err(guest_error("Cannot link an agent to itself"));
    }
    let signature_bytes: [u8; 64] = input
        .external_signature
        .try_into()
        .map_err(|_| guest_error("external_signature must be 64 bytes"))?;
    let external_signature = Signature(signature_bytes);

    let payload = sorted_agent_pair_bytes(&me, &input.external_agent);
    if !verify_signature_raw(
        input.external_agent.clone(),
        external_signature.clone(),
        payload.clone(),
    )? {
        return Err(guest_error(
            "The Flowsta Vault signature does not verify for this agent pair",
        ));
    }
    let my_signature = sign_raw(me.clone(), payload)?;

    let (agent_a, signature_a, agent_b, signature_b) =
        if me.get_raw_39() < input.external_agent.get_raw_39() {
            (me, my_signature, input.external_agent, external_signature)
        } else {
            (input.external_agent, external_signature, me, my_signature)
        };
    let attestation = IsSamePersonEntry {
        agent_a,
        signature_a,
        agent_b,
        signature_b,
        created_at: sys_time()?.as_micros() / 1_000_000,
    };

    let action_hash = create_entry(&EntryTypes::IsSamePerson(attestation.clone()))?;
    for agent in [&attestation.agent_a, &attestation.agent_b] {
        create_link(
            agent.clone(),
            action_hash.clone(),
            LinkTypes::AgentToIsSamePerson,
            (),
        )?;
    }
    Ok(action_hash)
}

/// Active (unrevoked) attestations involving `agent`, newest first.
#[hdk_extern]
pub fn get_identity_links(agent: AgentPubKey) -> ExternResult<Vec<IdentityLink>> {
    let links = get_links(
        LinkQuery::try_new(agent.clone(), LinkTypes::AgentToIsSamePerson)?,
        GetStrategy::Network,
    )?;
    let mut identity_links = Vec::new();
    let mut seen = HashSet::new();
    for link in links {
        let Some(action_hash) = link.target.into_action_hash() else {
            continue;
        };
        if !seen.insert(action_hash.clone()) {
            continue;
        }
        let Some(Details::Record(details)) = get_details(action_hash.clone(), GetOptions::default())?
        else {
            continue;
        };
        if !details.deletes.is_empty() {
            continue;
        }
        let Ok(Some(attestation)) = details.record.entry().to_app_option::<IsSamePersonEntry>()
        else {
            continue;
        };
        let Some(linked_agent) = attestation.other_agent(&agent).cloned() else {
            continue;
        };
        identity_links.push((
            details.record.action().timestamp(),
            IdentityLink {
                action_hash,
                linked_agent,
                attestation,
            },
        ));
    }
    identity_links.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(identity_links.into_iter().map(|(_, link)| link).collect())
}

/// Every agent linked to `agent` by an active attestation. Called by `@flowsta/holochain`'s
/// `getFlowstaIdentity`.
#[hdk_extern]
pub fn get_linked_agents(agent: AgentPubKey) -> ExternResult<Vec<AgentPubKey>> {
    let mut agents: Vec<AgentPubKey> = Vec::new();
    for link in get_identity_links(agent)? {
        if !agents.contains(&link.linked_agent) {
            agents.push(link.linked_agent);
        }
    }
    Ok(agents)
}

#[hdk_extern]
pub fn are_agents_linked(pair: AgentPair) -> ExternResult<bool> {
    Ok(get_linked_agents(pair.agent_a)?.contains(&pair.agent_b))
}

/// Revokes an attestation. Only one of its two agents may do this (enforced in validation).
#[hdk_extern]
pub fn revoke_link(attestation_hash: ActionHash) -> ExternResult<ActionHash> {
    let me = agent_info()?.agent_initial_pubkey;
    let record = get(attestation_hash.clone(), GetOptions::default())?
        .ok_or_else(|| guest_error("Attestation not found"))?;
    let attestation: IsSamePersonEntry = record
        .entry()
        .to_app_option()
        .map_err(|e| wasm_error!(e))?
        .ok_or_else(|| guest_error("Record is not an identity attestation"))?;
    if !attestation.involves(&me) {
        return Err(guest_error(
            "Only one of the two linked agents can revoke this attestation",
        ));
    }
    delete_entry(attestation_hash)
}
