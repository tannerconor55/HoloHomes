//! Integrity zome for Flowsta-compatible agent linking.
//!
//! Ported from WeAreFlowsta/flowsta-identity-dna v1.4 (Apache-2.0) to hdi 0.8. Beyond the
//! original, validation also enforces who may revoke an attestation and who may link to it,
//! both of which the original left to the coordinator.

use hdi::prelude::*;
pub use identity_attestation::*;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
#[hdk_entry_types]
#[unit_enum(UnitEntryTypes)]
pub enum EntryTypes {
    IsSamePerson(IsSamePersonEntry),
}

#[derive(Serialize, Deserialize)]
#[hdk_link_types]
pub enum LinkTypes {
    /// From each agent in an attestation to the attestation's create action.
    AgentToIsSamePerson,
}

#[hdk_extern]
pub fn validate(op: Op) -> ExternResult<ValidateCallbackResult> {
    match op.flattened::<EntryTypes, LinkTypes>()? {
        FlatOp::CreateEntry(OpEntry::CreateEntry { app_entry, action }) => {
            validate_create(action.author(), &app_entry)
        }
        FlatOp::CreateEntry(OpEntry::UpdateEntry { .. })
        | FlatOp::Update(OpUpdate::Entry { .. }) => reject_update(),
        FlatOp::Delete(OpDelete { action }) => validate_delete(action),
        FlatOp::Link(OpLink::CreateLink { link_type, action }) => {
            validate_create_link(link_type, action)
        }
        FlatOp::Link(OpLink::DeleteLink {
            original_action,
            action,
            ..
        }) => validate_delete_link(action, original_action),
        FlatOp::CreateRecord(record) => match record {
            OpRecord::CreateEntry { app_entry, action } => {
                validate_create(action.author(), &app_entry)
            }
            OpRecord::UpdateEntry { .. } => reject_update(),
            OpRecord::DeleteEntry { action, .. } => validate_delete(action),
            OpRecord::CreateLink { link_type, action } => validate_create_link(link_type, action),
            OpRecord::DeleteLink { action } => {
                let record = must_get_valid_record(action.link_add_address.clone())?;
                let create_link =
                    TypedAction::<CreateLinkData>::try_from_action(record.action().clone())?;
                validate_delete_link(action, create_link)
            }
            _ => Ok(ValidateCallbackResult::Valid),
        },
        _ => Ok(ValidateCallbackResult::Valid),
    }
}

fn validate_create(
    author: &AgentPubKey,
    entry: &EntryTypes,
) -> ExternResult<ValidateCallbackResult> {
    let EntryTypes::IsSamePerson(attestation) = entry;
    if let Err(reason) = check_attestation(attestation)? {
        return Ok(ValidateCallbackResult::Invalid(reason));
    }
    if !attestation.involves(author) {
        return Ok(ValidateCallbackResult::Invalid(
            "The author must be one of the two agents in the attestation".into(),
        ));
    }
    Ok(ValidateCallbackResult::Valid)
}

fn reject_update() -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(
        "Identity attestations cannot be updated; revoke and create a new one".into(),
    ))
}

fn validate_delete(action: TypedAction<DeleteData>) -> ExternResult<ValidateCallbackResult> {
    let record = must_get_valid_record(action.deletes_address.clone())?;
    let Ok(Some(attestation)) = record.entry().to_app_option::<IsSamePersonEntry>() else {
        // Not an attestation, so not this zome's to judge.
        return Ok(ValidateCallbackResult::Valid);
    };
    if !attestation.involves(action.author()) {
        return Ok(ValidateCallbackResult::Invalid(
            "Only one of the two linked agents can revoke an attestation".into(),
        ));
    }
    Ok(ValidateCallbackResult::Valid)
}

fn validate_create_link(
    link_type: LinkTypes,
    action: TypedAction<CreateLinkData>,
) -> ExternResult<ValidateCallbackResult> {
    match link_type {
        LinkTypes::AgentToIsSamePerson => {
            let Some(target) = action.target_address.clone().into_action_hash() else {
                return Ok(ValidateCallbackResult::Invalid(
                    "AgentToIsSamePerson links must target an action".into(),
                ));
            };
            let record = must_get_valid_record(target)?;
            let Ok(Some(attestation)) = record.entry().to_app_option::<IsSamePersonEntry>()
            else {
                return Ok(ValidateCallbackResult::Invalid(
                    "AgentToIsSamePerson links must target an identity attestation".into(),
                ));
            };
            let base_is_party = [&attestation.agent_a, &attestation.agent_b]
                .into_iter()
                .any(|agent| AnyLinkableHash::from(agent.clone()) == action.base_address);
            if !base_is_party {
                return Ok(ValidateCallbackResult::Invalid(
                    "AgentToIsSamePerson links must start from one of the linked agents".into(),
                ));
            }
            if !attestation.involves(action.author()) {
                return Ok(ValidateCallbackResult::Invalid(
                    "Only a linked agent can index an attestation".into(),
                ));
            }
            Ok(ValidateCallbackResult::Valid)
        }
    }
}

fn validate_delete_link(
    action: TypedAction<DeleteLinkData>,
    original_action: TypedAction<CreateLinkData>,
) -> ExternResult<ValidateCallbackResult> {
    if action.author() != original_action.author() {
        return Ok(ValidateCallbackResult::Invalid(
            "Only the link's author can delete it".into(),
        ));
    }
    Ok(ValidateCallbackResult::Valid)
}
