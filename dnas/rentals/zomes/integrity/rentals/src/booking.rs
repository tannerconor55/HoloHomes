use hdi::prelude::*;

use crate::{must_get_booking_request, must_get_listing, MAX_GUESTS_LIMIT};

pub const MICROS_PER_DAY: i64 = 86_400_000_000;
pub const MAX_STAY_DAYS: i64 = 365;
pub const MAX_MESSAGE_CHARS: usize = 2000;

/// A guest asking to stay. The host answers with a `BookingResponse`.
#[hdk_entry_helper]
#[derive(Clone, PartialEq)]
pub struct BookingRequest {
    /// The listing's original create action.
    pub listing_hash: ActionHash,
    pub check_in: Timestamp,
    pub check_out: Timestamp,
    pub guests: u32,
    pub message: String,
}

impl BookingRequest {
    /// Whether two requests for the same listing share at least one night.
    pub fn overlaps(&self, other: &BookingRequest) -> bool {
        self.listing_hash == other.listing_hash
            && self.check_in < other.check_out
            && other.check_in < self.check_out
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum BookingDecision {
    Accepted,
    Declined,
}

/// The host's answer to a booking request. Only the listing's host can author a valid one.
#[hdk_entry_helper]
#[derive(Clone, PartialEq)]
pub struct BookingResponse {
    pub request_hash: ActionHash,
    pub decision: BookingDecision,
    pub note: String,
}

pub fn validate_booking_request(
    action: &TypedAction<EntryCreationData>,
    request: &BookingRequest,
) -> ExternResult<ValidateCallbackResult> {
    let stay = request
        .check_out
        .as_micros()
        .checked_sub(request.check_in.as_micros());
    reject_if!(
        !matches!(stay, Some(micros) if micros >= MICROS_PER_DAY),
        "A stay must be at least one night"
    );
    reject_if!(
        stay.unwrap_or(i64::MAX) > MAX_STAY_DAYS * MICROS_PER_DAY,
        "A stay can be at most {MAX_STAY_DAYS} nights"
    );
    reject_if!(
        request.guests == 0 || request.guests > MAX_GUESTS_LIMIT,
        "guests must be between 1 and {MAX_GUESTS_LIMIT}"
    );
    reject_if!(
        request.message.chars().count() > MAX_MESSAGE_CHARS,
        "Booking messages are limited to {MAX_MESSAGE_CHARS} characters"
    );

    let (listing_record, _) = check!(must_get_listing(&request.listing_hash)?);
    reject_if!(
        !matches!(listing_record.action().data, ActionData::Create(_)),
        "listing_hash must be the listing's original create action"
    );
    reject_if!(
        listing_record.action().author() == action.author(),
        "Hosts cannot book their own listing"
    );
    Ok(ValidateCallbackResult::Valid)
}

pub fn validate_booking_response(
    action: &TypedAction<EntryCreationData>,
    response: &BookingResponse,
) -> ExternResult<ValidateCallbackResult> {
    reject_if!(
        response.note.chars().count() > MAX_MESSAGE_CHARS,
        "Response notes are limited to {MAX_MESSAGE_CHARS} characters"
    );
    let (_, request) = check!(must_get_booking_request(&response.request_hash)?);
    let (listing_record, _) = check!(must_get_listing(&request.listing_hash)?);
    reject_if!(
        listing_record.action().author() != action.author(),
        "Only the listing's host can respond to a booking request"
    );
    Ok(ValidateCallbackResult::Valid)
}
