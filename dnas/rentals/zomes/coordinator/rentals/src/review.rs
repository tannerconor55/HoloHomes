use hdk::prelude::*;
use identity_attestation::IsSamePersonEntry;
use rentals_integrity::*;

use crate::booking::booking_response_record;
use crate::{call_local_zome, decode_entry, guest_error, must_get_record, records_for_links};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CreateReviewInput {
    pub booking_request_hash: ActionHash,
    pub rating: u8,
    pub text: String,
}

/// Reviews a stay. If the guest has signed in with Flowsta Vault, their newest identity
/// attestation is attached so readers can see the review comes from a verified identity.
///
/// Validation enforces that the reviewer made the booking, the host accepted it, and check-out
/// has passed. The one-review-per-booking check here is best effort: two devices racing can
/// both pass it, since validation cannot prove that no other review exists.
#[hdk_extern]
pub fn create_review(input: CreateReviewInput) -> ExternResult<Record> {
    let me = agent_info()?.agent_initial_pubkey;
    let request_record = must_get_record(input.booking_request_hash.clone(), false)?;
    if request_record.action().author() != &me {
        return Err(guest_error("You can only review your own bookings"));
    }
    let request: BookingRequest = decode_entry(&request_record)?;
    let response_record = booking_response_record(&input.booking_request_hash)?
        .ok_or_else(|| guest_error("The host has not responded to this booking yet"))?;

    let existing = get_links(
        LinkQuery::try_new(
            input.booking_request_hash.clone(),
            LinkTypes::BookingRequestToReview,
        )?,
        GetStrategy::Network,
    )?;
    if !existing.is_empty() {
        return Err(guest_error("You have already reviewed this booking"));
    }

    let review = Review {
        booking_request_hash: input.booking_request_hash.clone(),
        booking_response_hash: response_record.action_address().clone(),
        rating: input.rating,
        text: input.text,
        identity_link_hash: my_identity_link(&me)?,
    };
    let review_hash = create_entry(&EntryTypes::Review(review))?;
    create_link(
        request.listing_hash,
        review_hash.clone(),
        LinkTypes::ListingToReviews,
        (),
    )?;
    create_link(
        input.booking_request_hash,
        review_hash.clone(),
        LinkTypes::BookingRequestToReview,
        (),
    )?;
    create_link(me, review_hash.clone(), LinkTypes::ReviewerToReviews, ())?;
    must_get_record(review_hash, true)
}

/// Mirrors the fields this zome needs from `agent_linking::IdentityLink`. A local mirror
/// avoids compiling the other coordinator's externs into this one.
#[derive(Serialize, Deserialize, Debug)]
struct IdentityLinkRef {
    action_hash: ActionHash,
}

fn my_identity_link(me: &AgentPubKey) -> ExternResult<Option<ActionHash>> {
    let links: Vec<IdentityLinkRef> =
        call_local_zome("agent_linking", "get_identity_links", me.clone())?;
    Ok(links.into_iter().next().map(|link| link.action_hash))
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ReviewView {
    pub review_hash: ActionHash,
    pub review: Review,
    pub reviewer: AgentPubKey,
    pub created_at: Timestamp,
    /// The Flowsta Vault identity attested for the reviewer, if that attestation still stands.
    pub flowsta_identity: Option<AgentPubKey>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RatingSummary {
    pub review_count: u32,
    pub average_rating: Option<f64>,
    pub verified_review_count: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ListingReviews {
    /// Newest first.
    pub reviews: Vec<ReviewView>,
    pub summary: RatingSummary,
}

/// A lighter rating readout for search results: just the count and average, skipping the
/// per-review Flowsta identity lookups `get_reviews_for_listing` does.
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct RatingOverview {
    pub review_count: u32,
    pub average_rating: Option<f64>,
}

pub(crate) fn rating_overview(listing_hash: &ActionHash) -> ExternResult<RatingOverview> {
    let links = get_links(
        LinkQuery::try_new(listing_hash.clone(), LinkTypes::ListingToReviews)?,
        GetStrategy::Network,
    )?;
    let mut total = 0u32;
    let mut count = 0u32;
    for record in records_for_links(links, false)? {
        let review: Review = decode_entry(&record)?;
        total += review.rating as u32;
        count += 1;
    }
    Ok(RatingOverview {
        review_count: count,
        average_rating: (count > 0).then(|| total as f64 / count as f64),
    })
}

#[hdk_extern]
pub fn get_reviews_for_listing(listing_hash: ActionHash) -> ExternResult<ListingReviews> {
    let links = get_links(
        LinkQuery::try_new(listing_hash, LinkTypes::ListingToReviews)?,
        GetStrategy::Network,
    )?;
    let mut reviews = Vec::new();
    for record in records_for_links(links, false)? {
        let review: Review = decode_entry(&record)?;
        let reviewer = record.action().author().clone();
        let flowsta_identity = match &review.identity_link_hash {
            Some(link_hash) => active_identity(link_hash, &reviewer)?,
            None => None,
        };
        reviews.push(ReviewView {
            review_hash: record.action_address().clone(),
            review,
            reviewer,
            created_at: record.action().timestamp(),
            flowsta_identity,
        });
    }
    reviews.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    let review_count = reviews.len() as u32;
    let total: u32 = reviews.iter().map(|view| view.review.rating as u32).sum();
    let summary = RatingSummary {
        review_count,
        average_rating: (review_count > 0).then(|| total as f64 / review_count as f64),
        verified_review_count: reviews
            .iter()
            .filter(|view| view.flowsta_identity.is_some())
            .count() as u32,
    };
    Ok(ListingReviews { reviews, summary })
}

/// The agent `reviewer` is linked to by the attestation at `link_hash`, unless it was revoked.
fn active_identity(link_hash: &ActionHash, reviewer: &AgentPubKey) -> ExternResult<Option<AgentPubKey>> {
    let Some(Details::Record(details)) = get_details(link_hash.clone(), GetOptions::default())? else {
        return Ok(None);
    };
    if !details.deletes.is_empty() {
        return Ok(None);
    }
    let Ok(Some(attestation)) = details.record.entry().to_app_option::<IsSamePersonEntry>() else {
        return Ok(None);
    };
    Ok(attestation.other_agent(reviewer).cloned())
}
