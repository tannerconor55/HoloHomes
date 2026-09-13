//! Integrity zome for HoloAirBNB: listings, their geohash/H3 index, bookings and reviews.
//!
//! Anything compiled into this crate, including `geo_utils` and `identity_attestation`,
//! is part of the DNA hash. Changing it creates a new, separate network.

use hdi::prelude::*;

/// Unwraps a `Result<T, String>`, turning `Err(reason)` into an `Invalid` validation result.
macro_rules! check {
    ($result:expr) => {
        match $result {
            Ok(value) => value,
            Err(reason) => return Ok(ValidateCallbackResult::Invalid(reason)),
        }
    };
}

/// Returns an `Invalid` validation result when the condition holds.
macro_rules! reject_if {
    ($condition:expr, $($reason:tt)+) => {
        if $condition {
            return Ok(ValidateCallbackResult::Invalid(format!($($reason)+)));
        }
    };
}

pub mod booking;
pub mod geo_index;
pub mod listing;
pub mod review;

pub use booking::*;
pub use geo_index::*;
pub use geo_utils;
pub use identity_attestation;
pub use listing::*;
pub use review::*;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
#[hdk_entry_types]
#[unit_enum(UnitEntryTypes)]
pub enum EntryTypes {
    Listing(Listing),
    BookingRequest(BookingRequest),
    BookingResponse(BookingResponse),
    Review(Review),
}

#[derive(Serialize, Deserialize)]
#[hdk_link_types]
pub enum LinkTypes {
    /// Geohash bucket path → listing create action. Tag: `GeoIndexTag`.
    GeohashToListing,
    /// H3 cell path → listing create action. Tag: `GeoIndexTag`.
    H3CellToListing,
    /// Host agent → listing create action.
    HostToListings,
    /// Listing create action → each of its update actions.
    ListingUpdates,
    /// Listing create action → booking request.
    ListingToBookingRequests,
    /// Guest agent → booking request.
    GuestToBookingRequests,
    /// Booking request → the host's response.
    BookingRequestToResponse,
    /// Listing create action → review.
    ListingToReviews,
    /// Booking request → its review.
    BookingRequestToReview,
    /// Reviewer agent → review.
    ReviewerToReviews,
}

// The network is open: anyone with the hApp can join. Identity is proven per action
// (Flowsta attestations attached to reviews), not at the membrane.
#[hdk_extern]
pub fn genesis_self_check(_data: GenesisSelfCheckData) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Valid)
}

pub fn validate_agent_joining(
    _agent_pub_key: AgentPubKey,
    _membrane_proof: &Option<MembraneProof>,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Valid)
}

// Record-level ops run the same checks as entry-level ops. Skipping them would let
// `must_get_valid_record` hand back records whose entry validation failed, and the booking
// and review rules below depend on that call meaning what it says.
#[hdk_extern]
pub fn validate(op: Op) -> ExternResult<ValidateCallbackResult> {
    match op.flattened::<EntryTypes, LinkTypes>()? {
        FlatOp::CreateEntry(OpEntry::CreateEntry { app_entry, action }) => {
            validate_create_entry(action.into(), app_entry)
        }
        FlatOp::CreateEntry(OpEntry::UpdateEntry {
            app_entry, action, ..
        }) => validate_entry_content(&action.into(), &app_entry),
        FlatOp::Update(OpUpdate::Entry { app_entry, action }) => validate_update(action, app_entry),
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
                validate_create_entry(action.into(), app_entry)
            }
            OpRecord::UpdateEntry {
                app_entry, action, ..
            } => match validate_entry_content(&action.clone().into(), &app_entry)? {
                ValidateCallbackResult::Valid => validate_update(action, app_entry),
                other => Ok(other),
            },
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
        FlatOp::AgentActivity(OpActivity::CreateAgent { agent, action }) => {
            let prev = action
                .prev_action()
                .ok_or_else(|| wasm_error!(WasmErrorInner::Guest("expected a prior action".into())))?
                .clone();
            let previous_action = must_get_action(prev)?;
            match &previous_action.action().data {
                ActionData::AgentValidationPkg(AgentValidationPkgData { membrane_proof, .. }) => {
                    validate_agent_joining(agent, membrane_proof)
                }
                _ => invalid(
                    "The previous action for a `CreateAgent` action must be an `AgentValidationPkg`",
                ),
            }
        }
        _ => Ok(ValidateCallbackResult::Valid),
    }
}

pub(crate) fn invalid(reason: impl Into<String>) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(reason.into()))
}

fn validate_create_entry(
    action: TypedAction<EntryCreationData>,
    entry: EntryTypes,
) -> ExternResult<ValidateCallbackResult> {
    match validate_entry_content(&action, &entry)? {
        ValidateCallbackResult::Valid => {}
        other => return Ok(other),
    }
    if let EntryTypes::Listing(listing) = &entry {
        reject_if!(
            listing.status != ListingStatus::Active,
            "A new listing must be active"
        );
    }
    Ok(ValidateCallbackResult::Valid)
}

/// Rules every version of an entry must satisfy, whether created or updated.
fn validate_entry_content(
    action: &TypedAction<EntryCreationData>,
    entry: &EntryTypes,
) -> ExternResult<ValidateCallbackResult> {
    match entry {
        EntryTypes::Listing(listing) => validate_listing(listing),
        EntryTypes::BookingRequest(request) => validate_booking_request(action, request),
        EntryTypes::BookingResponse(response) => validate_booking_response(action, response),
        EntryTypes::Review(review) => validate_review(action, review),
    }
}

fn validate_update(
    action: TypedAction<UpdateData>,
    entry: EntryTypes,
) -> ExternResult<ValidateCallbackResult> {
    let previous_record = must_get_valid_record(action.original_action_address.clone())?;
    reject_if!(
        action.author() != previous_record.action().author(),
        "Only the original author can update this entry"
    );
    match entry {
        EntryTypes::Listing(listing) => {
            let Ok(Some(previous)) = previous_record.entry().to_app_option::<Listing>() else {
                return invalid("A listing can only replace a listing");
            };
            validate_listing_update(&listing, &previous)
        }
        EntryTypes::BookingRequest(_) => {
            invalid("Booking requests cannot be edited; withdraw the request and send a new one")
        }
        EntryTypes::BookingResponse(_) => invalid("Booking responses are final"),
        EntryTypes::Review(_) => invalid("Reviews cannot be edited"),
    }
}

fn validate_delete(action: TypedAction<DeleteData>) -> ExternResult<ValidateCallbackResult> {
    let (record, entry) = match must_get_rentals_entry(&action.deletes_address)? {
        Ok(found) => found,
        // Not one of this zome's entry types, so not ours to judge.
        Err(_) => return Ok(ValidateCallbackResult::Valid),
    };
    reject_if!(
        action.author() != record.action().author(),
        "Only the original author can delete this entry"
    );
    match entry {
        EntryTypes::Listing(_) | EntryTypes::BookingRequest(_) => {
            Ok(ValidateCallbackResult::Valid)
        }
        EntryTypes::BookingResponse(_) => invalid("Booking responses are permanent"),
        EntryTypes::Review(_) => invalid("Reviews are permanent"),
    }
}

fn validate_create_link(
    link_type: LinkTypes,
    action: TypedAction<CreateLinkData>,
) -> ExternResult<ValidateCallbackResult> {
    let author = action.author().clone();
    let author_base = AnyLinkableHash::from(author.clone());
    let Some(target) = action.target_address.clone().into_action_hash() else {
        return invalid("Rentals links must target an action hash");
    };
    match link_type {
        LinkTypes::GeohashToListing | LinkTypes::H3CellToListing => {
            validate_geo_index_link(&link_type, &action, &target)
        }
        LinkTypes::HostToListings => {
            let (record, _) = check!(must_get_listing(&target)?);
            reject_if!(
                action.base_address != author_base,
                "HostToListings links must start from the host's own key"
            );
            reject_if!(
                record.action().author() != &author,
                "Hosts can only index their own listings"
            );
            Ok(ValidateCallbackResult::Valid)
        }
        LinkTypes::ListingUpdates => {
            let Some(base) = action.base_address.clone().into_action_hash() else {
                return invalid("ListingUpdates links must start from a listing");
            };
            let (original, _) = check!(must_get_listing(&base)?);
            let (update, _) = check!(must_get_listing(&target)?);
            reject_if!(
                original.action().author() != &author || update.action().author() != &author,
                "Only the host can link updates to their listing"
            );
            Ok(ValidateCallbackResult::Valid)
        }
        LinkTypes::ListingToBookingRequests => {
            let (record, request) = check!(must_get_booking_request(&target)?);
            reject_if!(
                action.base_address != AnyLinkableHash::from(request.listing_hash),
                "Booking request links must start from the booked listing"
            );
            reject_if!(
                record.action().author() != &author,
                "Only the guest can link their booking request"
            );
            Ok(ValidateCallbackResult::Valid)
        }
        LinkTypes::GuestToBookingRequests => {
            let (record, _) = check!(must_get_booking_request(&target)?);
            reject_if!(
                action.base_address != author_base || record.action().author() != &author,
                "Guests can only index their own booking requests under their own key"
            );
            Ok(ValidateCallbackResult::Valid)
        }
        LinkTypes::BookingRequestToResponse => {
            let (record, response) = check!(must_get_booking_response(&target)?);
            reject_if!(
                action.base_address != AnyLinkableHash::from(response.request_hash),
                "Booking response links must start from the request they answer"
            );
            reject_if!(
                record.action().author() != &author,
                "Only the host can link their response"
            );
            Ok(ValidateCallbackResult::Valid)
        }
        LinkTypes::ListingToReviews => {
            let (record, review) = check!(must_get_review(&target)?);
            let (_, request) = check!(must_get_booking_request(&review.booking_request_hash)?);
            reject_if!(
                action.base_address != AnyLinkableHash::from(request.listing_hash),
                "Review links must start from the reviewed listing"
            );
            reject_if!(
                record.action().author() != &author,
                "Only the reviewer can link their review"
            );
            Ok(ValidateCallbackResult::Valid)
        }
        LinkTypes::BookingRequestToReview => {
            let (record, review) = check!(must_get_review(&target)?);
            reject_if!(
                action.base_address != AnyLinkableHash::from(review.booking_request_hash),
                "Review links must start from the reviewed booking request"
            );
            reject_if!(
                record.action().author() != &author,
                "Only the reviewer can link their review"
            );
            Ok(ValidateCallbackResult::Valid)
        }
        LinkTypes::ReviewerToReviews => {
            let (record, _) = check!(must_get_review(&target)?);
            reject_if!(
                action.base_address != author_base || record.action().author() != &author,
                "Reviewers can only index their own reviews under their own key"
            );
            Ok(ValidateCallbackResult::Valid)
        }
    }
}

fn validate_delete_link(
    action: TypedAction<DeleteLinkData>,
    original_action: TypedAction<CreateLinkData>,
) -> ExternResult<ValidateCallbackResult> {
    reject_if!(
        action.author() != original_action.author(),
        "Only the link's author can delete it"
    );
    Ok(ValidateCallbackResult::Valid)
}

// ── Fetching referenced entries ─────────────────────────────────────────

/// Fetches a valid record and decodes it as one of this zome's entry types.
/// The inner `Err` is a reason to reject the op that referenced it.
pub fn must_get_rentals_entry(
    hash: &ActionHash,
) -> ExternResult<Result<(Record, EntryTypes), String>> {
    let record = must_get_valid_record(hash.clone())?;
    let Some(EntryType::App(def)) = record.action().entry_type() else {
        return Ok(Err(format!("{hash} is not an app entry")));
    };
    let Some(entry) = record.entry().as_option() else {
        return Ok(Err(format!("{hash} carries no entry")));
    };
    let decoded = EntryTypes::deserialize_from_type(def.zome_index, def.entry_index, entry)?;
    Ok(match decoded {
        Some(app_entry) => Ok((record, app_entry)),
        None => Err(format!("{hash} is not a rentals entry")),
    })
}

macro_rules! typed_getter {
    ($fn_name:ident, $variant:ident, $what:literal) => {
        #[doc = concat!("Fetches a valid record that must hold a ", $what, ".")]
        pub fn $fn_name(hash: &ActionHash) -> ExternResult<Result<(Record, $variant), String>> {
            Ok(match must_get_rentals_entry(hash)? {
                Ok((record, EntryTypes::$variant(entry))) => Ok((record, entry)),
                Ok(_) => Err(format!("{hash} is not a {}", $what)),
                Err(reason) => Err(reason),
            })
        }
    };
}

typed_getter!(must_get_listing, Listing, "listing");
typed_getter!(must_get_booking_request, BookingRequest, "booking request");
typed_getter!(must_get_booking_response, BookingResponse, "booking response");
typed_getter!(must_get_review, Review, "review");
