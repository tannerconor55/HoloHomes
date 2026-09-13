pub mod booking;
pub mod listing;
pub mod review;
pub mod search;

use hdk::prelude::*;
use rentals_integrity::*;

#[hdk_extern]
pub fn init() -> ExternResult<InitCallbackResult> {
    Ok(InitCallbackResult::Pass)
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum Signal {
    LinkCreated {
        action: SignedActionHashed,
        link_type: LinkTypes,
    },
    LinkDeleted {
        action: SignedActionHashed,
        create_link_action: SignedActionHashed,
        link_type: LinkTypes,
    },
    EntryCreated {
        action: SignedActionHashed,
        app_entry: EntryTypes,
    },
    EntryUpdated {
        action: SignedActionHashed,
        app_entry: EntryTypes,
        original_app_entry: EntryTypes,
    },
    EntryDeleted {
        action: SignedActionHashed,
        original_app_entry: EntryTypes,
    },
}

// Signals come from post_commit so the UI only ever sees actions that actually committed.
#[hdk_extern(infallible)]
pub fn post_commit(committed_actions: Vec<SignedActionHashed>) {
    for action in committed_actions {
        if let Err(err) = signal_action(action) {
            error!("Error signaling new action: {:?}", err);
        }
    }
}

fn signal_action(action: SignedActionHashed) -> ExternResult<()> {
    match &action.hashed.content.clone().data {
        ActionData::CreateLink(create_link) => {
            if let Ok(Some(link_type)) =
                LinkTypes::from_type(create_link.zome_index, create_link.link_type)
            {
                emit_signal(Signal::LinkCreated { action, link_type })?;
            }
            Ok(())
        }
        ActionData::DeleteLink(delete_link) => {
            let record = get(delete_link.link_add_address.clone(), GetOptions::default())?
                .ok_or_else(|| guest_error("Failed to fetch CreateLink action"))?;
            match &record.action().data {
                ActionData::CreateLink(create_link) => {
                    if let Ok(Some(link_type)) =
                        LinkTypes::from_type(create_link.zome_index, create_link.link_type)
                    {
                        emit_signal(Signal::LinkDeleted {
                            action,
                            link_type,
                            create_link_action: record.signed_action.clone(),
                        })?;
                    }
                    Ok(())
                }
                _ => Err(guest_error("Create Link should exist")),
            }
        }
        ActionData::Create(_) => {
            if let Ok(Some(app_entry)) = get_entry_for_action(&action.hashed.hash) {
                emit_signal(Signal::EntryCreated { action, app_entry })?;
            }
            Ok(())
        }
        ActionData::Update(update) => {
            if let Ok(Some(app_entry)) = get_entry_for_action(&action.hashed.hash) {
                if let Ok(Some(original_app_entry)) =
                    get_entry_for_action(&update.original_action_address)
                {
                    emit_signal(Signal::EntryUpdated {
                        action,
                        app_entry,
                        original_app_entry,
                    })?;
                }
            }
            Ok(())
        }
        ActionData::Delete(delete) => {
            if let Ok(Some(original_app_entry)) = get_entry_for_action(&delete.deletes_address) {
                emit_signal(Signal::EntryDeleted {
                    action,
                    original_app_entry,
                })?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn get_entry_for_action(action_hash: &ActionHash) -> ExternResult<Option<EntryTypes>> {
    match get_details(action_hash.clone(), GetOptions::default())? {
        Some(Details::Record(details)) => rentals_entry(&details.record),
        _ => Ok(None),
    }
}

// ── Shared helpers ──────────────────────────────────────────────────────

pub(crate) fn guest_error(message: impl Into<String>) -> WasmError {
    wasm_error!(WasmErrorInner::Guest(message.into()))
}

pub(crate) fn get_options(local_only: bool) -> GetOptions {
    if local_only {
        GetOptions::default().with_strategy(GetStrategy::Local)
    } else {
        GetOptions::default()
    }
}

pub(crate) fn link_strategy(local_only: bool) -> GetStrategy {
    if local_only {
        GetStrategy::Local
    } else {
        GetStrategy::Network
    }
}

pub(crate) fn must_get_record(hash: ActionHash, local_only: bool) -> ExternResult<Record> {
    get(hash.clone(), get_options(local_only))?.ok_or_else(|| {
        guest_error(format!(
            "Record {hash} not found (it may not have reached this node yet)"
        ))
    })
}

/// Decodes a record as one of this zome's entry types, or `None` for anything else.
pub(crate) fn rentals_entry(record: &Record) -> ExternResult<Option<EntryTypes>> {
    let Some(EntryType::App(def)) = record.action().entry_type() else {
        return Ok(None);
    };
    let Some(entry) = record.entry().as_option() else {
        return Ok(None);
    };
    EntryTypes::deserialize_from_type(def.zome_index, def.entry_index, entry)
}

pub(crate) fn decode_entry<T>(record: &Record) -> ExternResult<T>
where
    T: TryFrom<SerializedBytes, Error = SerializedBytesError>,
{
    record
        .entry()
        .to_app_option::<T>()
        .map_err(|e| wasm_error!(e))?
        .ok_or_else(|| guest_error(format!("Record {} has no entry", record.action_address())))
}

pub(crate) fn records_for_links(links: Vec<Link>, local_only: bool) -> ExternResult<Vec<Record>> {
    let inputs: Vec<GetInput> = links
        .into_iter()
        .filter_map(|link| link.target.into_action_hash())
        .map(|hash| GetInput::new(hash.into(), get_options(local_only)))
        .collect();
    let records = HDK.with(|hdk| hdk.borrow().get(inputs))?;
    Ok(records.into_iter().flatten().collect())
}

/// Calls a function in another zome of this cell.
pub(crate) fn call_local_zome<I, O>(zome_name: &str, fn_name: &str, payload: I) -> ExternResult<O>
where
    I: serde::Serialize + std::fmt::Debug,
    O: serde::de::DeserializeOwned + std::fmt::Debug,
{
    match call(
        CallTargetCell::Local,
        zome_name,
        fn_name.into(),
        None,
        payload,
    )? {
        ZomeCallResponse::Ok(result) => result.decode().map_err(|e| {
            guest_error(format!("Could not decode {zome_name}/{fn_name} response: {e:?}"))
        }),
        ZomeCallResponse::Unauthorized(auth, _, zome, func) => Err(guest_error(format!(
            "Unauthorized: {zome}/{func} ({auth:?})"
        ))),
        ZomeCallResponse::AuthenticationFailed(_, agent) => Err(guest_error(format!(
            "Authentication failed for {agent:?}"
        ))),
        ZomeCallResponse::NetworkError(e) => Err(guest_error(format!("Network error: {e}"))),
        ZomeCallResponse::CountersigningSession(e) => Err(guest_error(format!(
            "Countersigning session failed to start: {e}"
        ))),
    }
}
