use hdi::prelude::*;
use identity_attestation::{check_attestation, IsSamePersonEntry};

use crate::{must_get_booking_request, must_get_booking_response, BookingDecision};

pub const MAX_REVIEW_CHARS: usize = 2000;

/// A guest's review of a completed stay.
#[hdk_entry_helper]
#[derive(Clone, PartialEq)]
pub struct Review {
    pub booking_request_hash: ActionHash,
    /// The host's acceptance of `booking_request_hash`.
    pub booking_response_hash: ActionHash,
    /// 1 to 5 stars.
    pub rating: u8,
    pub text: String,
    /// An attestation linking the reviewer to a Flowsta Vault identity, if they signed in with one.
    pub identity_link_hash: Option<ActionHash>,
}

/// Everything here is checkable from DHT data, so a review that passes is provably from a
/// guest whose booking the host accepted, written after that stay ended.
pub fn validate_review(
    action: &TypedAction<EntryCreationData>,
    review: &Review,
) -> ExternResult<ValidateCallbackResult> {
    reject_if!(
        !(1..=5).contains(&review.rating),
        "Ratings must be between 1 and 5"
    );
    reject_if!(
        review.text.chars().count() > MAX_REVIEW_CHARS,
        "Reviews are limited to {MAX_REVIEW_CHARS} characters"
    );

    let (request_record, request) = check!(must_get_booking_request(&review.booking_request_hash)?);
    reject_if!(
        request_record.action().author() != action.author(),
        "Only the guest who made the booking can review it"
    );
    let (_, response) = check!(must_get_booking_response(&review.booking_response_hash)?);
    reject_if!(
        response.request_hash != review.booking_request_hash,
        "booking_response_hash does not answer booking_request_hash"
    );
    reject_if!(
        response.decision != BookingDecision::Accepted,
        "Only accepted bookings can be reviewed"
    );
    // The action timestamp is signed by the author and checked by the network, unlike a clock read.
    reject_if!(
        action.timestamp() < request.check_out,
        "Reviews can only be written after check-out"
    );

    if let Some(link_hash) = &review.identity_link_hash {
        let link_record = must_get_valid_record(link_hash.clone())?;
        let Ok(Some(attestation)) = link_record.entry().to_app_option::<IsSamePersonEntry>() else {
            return Ok(ValidateCallbackResult::Invalid(
                "identity_link_hash is not an identity attestation".into(),
            ));
        };
        check!(check_attestation(&attestation)?);
        reject_if!(
            !attestation.involves(action.author()),
            "The identity attestation does not include the reviewer"
        );
    }
    Ok(ValidateCallbackResult::Valid)
}
