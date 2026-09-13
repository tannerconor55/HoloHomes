use geo_utils::{geohash, h3_cell, GeoPoint, LISTING_GEOHASH_PRECISION, LISTING_H3_RESOLUTION};
use hdi::prelude::*;

pub const MAX_TITLE_CHARS: usize = 120;
pub const MAX_DESCRIPTION_CHARS: usize = 5000;
pub const MAX_GUESTS_LIMIT: u32 = 50;

#[hdk_entry_helper]
#[derive(Clone, PartialEq)]
pub struct Listing {
    pub title: String,
    pub description: String,
    /// Public on the DHT. Hosts who want privacy should publish an approximate point and
    /// share the exact address once a booking is accepted.
    pub location: GeoPoint,
    /// Geohash of `location` at `LISTING_GEOHASH_PRECISION`. Validation recomputes it.
    pub geohash: String,
    /// H3 cell of `location` at `LISTING_H3_RESOLUTION`, as a hex string. Validation recomputes it.
    pub h3_cell: String,
    pub max_guests: u32,
    pub status: ListingStatus,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListingStatus {
    Active,
    /// Hidden from search and closed to new bookings. Existing bookings and reviews stay.
    Archived,
}

pub fn validate_listing(listing: &Listing) -> ExternResult<ValidateCallbackResult> {
    reject_if!(listing.title.trim().is_empty(), "A listing needs a title");
    reject_if!(
        listing.title.chars().count() > MAX_TITLE_CHARS,
        "Listing titles are limited to {MAX_TITLE_CHARS} characters"
    );
    reject_if!(
        listing.description.chars().count() > MAX_DESCRIPTION_CHARS,
        "Listing descriptions are limited to {MAX_DESCRIPTION_CHARS} characters"
    );
    reject_if!(
        listing.max_guests == 0 || listing.max_guests > MAX_GUESTS_LIMIT,
        "max_guests must be between 1 and {MAX_GUESTS_LIMIT}"
    );
    check!(listing.location.check().map_err(|e| e.to_string()));

    // Recomputing both encodings stops a host from filing a listing under the wrong place.
    let expected_geohash = check!(
        geohash(listing.location, LISTING_GEOHASH_PRECISION).map_err(|e| e.to_string())
    );
    reject_if!(
        listing.geohash != expected_geohash,
        "geohash {} does not match the location (expected {expected_geohash})",
        listing.geohash
    );
    let expected_h3 = check!(
        h3_cell(listing.location, LISTING_H3_RESOLUTION).map_err(|e| e.to_string())
    )
    .to_string();
    reject_if!(
        listing.h3_cell != expected_h3,
        "h3_cell {} does not match the location (expected {expected_h3})",
        listing.h3_cell
    );
    Ok(ValidateCallbackResult::Valid)
}

/// Location is fixed for the life of a listing, so its geo index links never go stale.
pub fn validate_listing_update(
    listing: &Listing,
    previous: &Listing,
) -> ExternResult<ValidateCallbackResult> {
    reject_if!(
        listing.location != previous.location,
        "A listing's location cannot change; archive it and create a new listing"
    );
    Ok(ValidateCallbackResult::Valid)
}
