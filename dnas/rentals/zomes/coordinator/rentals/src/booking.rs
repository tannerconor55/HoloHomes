use hdk::prelude::*;
use rentals_integrity::*;

use crate::listing::latest_listing_record;
use crate::{
    decode_entry, guest_error, link_strategy, must_get_record, records_for_links, rentals_entry,
};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RequestBookingInput {
    /// The listing's original create action (`ListingMatch::listing_hash`).
    pub listing_hash: ActionHash,
    pub check_in: Timestamp,
    pub check_out: Timestamp,
    pub guests: u32,
    #[serde(default)]
    pub message: String,
}

#[hdk_extern]
pub fn request_booking(input: RequestBookingInput) -> ExternResult<Record> {
    let listing_record = latest_listing_record(input.listing_hash.clone(), false)?
        .ok_or_else(|| guest_error("Listing not found"))?;
    let listing: Listing = decode_entry(&listing_record)?;
    if listing.status != ListingStatus::Active {
        return Err(guest_error("This listing is not accepting bookings"));
    }
    if input.guests > listing.max_guests {
        return Err(guest_error(format!(
            "This listing hosts at most {} guests",
            listing.max_guests
        )));
    }

    let request = BookingRequest {
        listing_hash: input.listing_hash.clone(),
        check_in: input.check_in,
        check_out: input.check_out,
        guests: input.guests,
        message: input.message,
    };
    let request_hash = create_entry(&EntryTypes::BookingRequest(request))?;
    create_link(
        input.listing_hash,
        request_hash.clone(),
        LinkTypes::ListingToBookingRequests,
        (),
    )?;
    create_link(
        agent_info()?.agent_initial_pubkey,
        request_hash.clone(),
        LinkTypes::GuestToBookingRequests,
        (),
    )?;
    must_get_record(request_hash, true)
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RespondToBookingInput {
    pub request_hash: ActionHash,
    pub decision: BookingDecision,
    #[serde(default)]
    pub note: String,
}

/// The host accepts or declines a request. Accepting is refused if the dates overlap a
/// booking this host already accepted for the same listing.
#[hdk_extern]
pub fn respond_to_booking(input: RespondToBookingInput) -> ExternResult<Record> {
    let me = agent_info()?.agent_initial_pubkey;
    let request: BookingRequest = decode_entry(&must_get_record(input.request_hash.clone(), false)?)?;
    let listing_record = must_get_record(request.listing_hash.clone(), false)?;
    if listing_record.action().author() != &me {
        return Err(guest_error(
            "Only the listing's host can respond to this booking request",
        ));
    }

    let my_responses = my_booking_responses()?;
    if my_responses
        .iter()
        .any(|response| response.request_hash == input.request_hash)
    {
        return Err(guest_error("You have already responded to this booking request"));
    }
    if input.decision == BookingDecision::Accepted {
        for response in my_responses
            .iter()
            .filter(|response| response.decision == BookingDecision::Accepted)
        {
            let accepted: BookingRequest =
                decode_entry(&must_get_record(response.request_hash.clone(), false)?)?;
            if accepted.overlaps(&request) {
                return Err(guest_error(format!(
                    "These dates overlap booking {} that you already accepted",
                    response.request_hash
                )));
            }
        }
    }

    let response_hash = create_entry(&EntryTypes::BookingResponse(BookingResponse {
        request_hash: input.request_hash.clone(),
        decision: input.decision,
        note: input.note,
    }))?;
    create_link(
        input.request_hash,
        response_hash.clone(),
        LinkTypes::BookingRequestToResponse,
        (),
    )?;
    must_get_record(response_hash, true)
}

/// Every response this agent has authored, read from its own source chain. Only a listing's
/// host can author a valid response to it, so for double-booking checks this local read is
/// complete: no other agent can accept a booking on the host's behalf.
fn my_booking_responses() -> ExternResult<Vec<BookingResponse>> {
    let filter = ChainQueryFilter::new()
        .action_type(ActionType::Create)
        .include_entries(true);
    let mut responses = Vec::new();
    for record in query(filter)? {
        if let Some(EntryTypes::BookingResponse(response)) = rentals_entry(&record)? {
            responses.push(response);
        }
    }
    Ok(responses)
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BookingView {
    pub request_hash: ActionHash,
    pub request_record: Record,
    pub request: BookingRequest,
    pub response_hash: Option<ActionHash>,
    pub response: Option<BookingResponse>,
}

/// The host's response to a booking request, if one has arrived.
pub(crate) fn booking_response_record(request_hash: &ActionHash) -> ExternResult<Option<Record>> {
    let links = get_links(
        LinkQuery::try_new(request_hash.clone(), LinkTypes::BookingRequestToResponse)?,
        GetStrategy::Network,
    )?;
    // Validation only admits responses authored by the host, and the host's coordinator
    // writes one per request; take the earliest in case of a race between devices.
    let Some(first) = links.into_iter().min_by(|a, b| a.timestamp.cmp(&b.timestamp)) else {
        return Ok(None);
    };
    let Some(response_hash) = first.target.into_action_hash() else {
        return Ok(None);
    };
    get(response_hash, GetOptions::default())
}

fn booking_view(request_record: Record) -> ExternResult<BookingView> {
    let request_hash = request_record.action_address().clone();
    let request: BookingRequest = decode_entry(&request_record)?;
    let response_record = booking_response_record(&request_hash)?;
    let response = response_record
        .as_ref()
        .map(decode_entry::<BookingResponse>)
        .transpose()?;
    Ok(BookingView {
        request_hash,
        request_record,
        request,
        response_hash: response_record.map(|record| record.action_address().clone()),
        response,
    })
}

#[hdk_extern]
pub fn get_booking(request_hash: ActionHash) -> ExternResult<Option<BookingView>> {
    get(request_hash, GetOptions::default())?
        .map(booking_view)
        .transpose()
}

/// An accepted booking's dates. Only ever built from `BookingDecision::Accepted` responses:
/// a pending request does not block a date, and only one response per request is ever valid.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct BusyRange {
    pub check_in: Timestamp,
    pub check_out: Timestamp,
}

pub(crate) fn ranges_overlap(
    a_start: Timestamp,
    a_end: Timestamp,
    b_start: Timestamp,
    b_end: Timestamp,
) -> bool {
    a_start < b_end && b_start < a_end
}

/// The accepted (and only the accepted) booking date ranges for a listing, for showing
/// guests what is already taken and for the `available_check_in`/`available_check_out`
/// search filter. Carries no guest identity — safe to expose to anyone.
pub(crate) fn busy_ranges_for_listing(listing_hash: &ActionHash) -> ExternResult<Vec<BusyRange>> {
    let links = get_links(
        LinkQuery::try_new(listing_hash.clone(), LinkTypes::ListingToBookingRequests)?,
        GetStrategy::Network,
    )?;
    let mut ranges = Vec::new();
    for record in records_for_links(links, false)? {
        let request_hash = record.action_address().clone();
        let request: BookingRequest = decode_entry(&record)?;
        let Some(response_record) = booking_response_record(&request_hash)? else {
            continue;
        };
        let response: BookingResponse = decode_entry(&response_record)?;
        if response.decision == BookingDecision::Accepted {
            ranges.push(BusyRange {
                check_in: request.check_in,
                check_out: request.check_out,
            });
        }
    }
    Ok(ranges)
}

#[hdk_extern]
pub fn get_busy_ranges_for_listing(listing_hash: ActionHash) -> ExternResult<Vec<BusyRange>> {
    busy_ranges_for_listing(&listing_hash)
}

/// All booking requests for a listing, with the host's responses. For the host's inbox.
#[hdk_extern]
pub fn get_booking_requests_for_listing(listing_hash: ActionHash) -> ExternResult<Vec<BookingView>> {
    let links = get_links(
        LinkQuery::try_new(listing_hash, LinkTypes::ListingToBookingRequests)?,
        GetStrategy::Network,
    )?;
    records_for_links(links, false)?
        .into_iter()
        .map(booking_view)
        .collect()
}

/// This agent's own booking requests, with any responses.
#[hdk_extern]
pub fn get_my_bookings() -> ExternResult<Vec<BookingView>> {
    let links = get_links(
        LinkQuery::try_new(
            agent_info()?.agent_initial_pubkey,
            LinkTypes::GuestToBookingRequests,
        )?,
        link_strategy(true),
    )?;
    records_for_links(links, true)?
        .into_iter()
        .map(booking_view)
        .collect()
}
