use geo_utils::{
    geohash, geohash_index_cells, h3_cell, h3_index_cells, GeoError, GeoPoint,
    LISTING_GEOHASH_PRECISION, LISTING_H3_RESOLUTION,
};
use hdk::prelude::*;
use rentals_integrity::*;

use crate::{decode_entry, guest_error, link_strategy, must_get_record, records_for_links};

pub(crate) fn geo_error(e: GeoError) -> WasmError {
    guest_error(e.to_string())
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CreateListingInput {
    pub title: String,
    pub description: String,
    pub location: GeoPoint,
    pub max_guests: u32,
}

/// Creates a listing and files it under its geohash and H3 index cells.
#[hdk_extern]
pub fn create_listing(input: CreateListingInput) -> ExternResult<Record> {
    let location = input.location;
    let listing = Listing {
        title: input.title,
        description: input.description,
        location,
        geohash: geohash(location, LISTING_GEOHASH_PRECISION).map_err(geo_error)?,
        h3_cell: h3_cell(location, LISTING_H3_RESOLUTION)
            .map_err(geo_error)?
            .to_string(),
        max_guests: input.max_guests,
        status: ListingStatus::Active,
    };
    let listing_hash = create_entry(&EntryTypes::Listing(listing))?;

    let tag = GeoIndexTag { location }
        .to_link_tag()
        .map_err(|e| wasm_error!(e))?;
    for cell in geohash_index_cells(location).map_err(geo_error)? {
        create_link(
            geohash_bucket_path(&cell).path_entry_hash()?,
            listing_hash.clone(),
            LinkTypes::GeohashToListing,
            tag.clone(),
        )?;
    }
    for cell in h3_index_cells(location).map_err(geo_error)? {
        create_link(
            h3_bucket_path(cell).path_entry_hash()?,
            listing_hash.clone(),
            LinkTypes::H3CellToListing,
            tag.clone(),
        )?;
    }
    create_link(
        agent_info()?.agent_initial_pubkey,
        listing_hash.clone(),
        LinkTypes::HostToListings,
        (),
    )?;
    must_get_record(listing_hash, true)
}

/// The newest version of a listing, given its original create action.
pub(crate) fn latest_listing_record(
    original_listing_hash: ActionHash,
    local_only: bool,
) -> ExternResult<Option<Record>> {
    let links = get_links(
        LinkQuery::try_new(original_listing_hash.clone(), LinkTypes::ListingUpdates)?,
        link_strategy(local_only),
    )?;
    let latest_hash = links
        .into_iter()
        .max_by(|a, b| a.timestamp.cmp(&b.timestamp))
        .and_then(|link| link.target.into_action_hash())
        .unwrap_or(original_listing_hash);
    get(latest_hash, crate::get_options(local_only))
}

#[hdk_extern]
pub fn get_listing(original_listing_hash: ActionHash) -> ExternResult<Option<Record>> {
    latest_listing_record(original_listing_hash, false)
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ListingView {
    /// The listing's original create action, which bookings and updates refer to.
    pub listing_hash: ActionHash,
    /// The latest version of the listing.
    pub record: Record,
    pub listing: Listing,
}

/// Latest versions of every listing `host` has published.
#[hdk_extern]
pub fn get_listings_for_host(host: AgentPubKey) -> ExternResult<Vec<ListingView>> {
    listings_for_host(host, false)
}

#[hdk_extern]
pub fn get_my_listings() -> ExternResult<Vec<ListingView>> {
    listings_for_host(agent_info()?.agent_initial_pubkey, true)
}

fn listings_for_host(host: AgentPubKey, local_only: bool) -> ExternResult<Vec<ListingView>> {
    let links = get_links(
        LinkQuery::try_new(host, LinkTypes::HostToListings)?,
        link_strategy(local_only),
    )?;
    let mut views = Vec::new();
    for original in records_for_links(links, local_only)? {
        let listing_hash = original.action_address().clone();
        if let Some(record) = latest_listing_record(listing_hash.clone(), local_only)? {
            let listing = decode_entry(&record)?;
            views.push(ListingView {
                listing_hash,
                record,
                listing,
            });
        }
    }
    Ok(views)
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UpdateListingInput {
    pub original_listing_hash: ActionHash,
    pub previous_listing_hash: ActionHash,
    pub title: String,
    pub description: String,
    pub max_guests: u32,
    pub status: ListingStatus,
}

/// Updates everything except the location, which is fixed so the geo index stays correct.
#[hdk_extern]
pub fn update_listing(input: UpdateListingInput) -> ExternResult<Record> {
    let previous: Listing = decode_entry(&must_get_record(input.previous_listing_hash.clone(), false)?)?;
    let updated = Listing {
        title: input.title,
        description: input.description,
        max_guests: input.max_guests,
        status: input.status,
        ..previous
    };
    let updated_hash = update_entry(input.previous_listing_hash, &updated)?;
    create_link(
        input.original_listing_hash,
        updated_hash.clone(),
        LinkTypes::ListingUpdates,
        (),
    )?;
    must_get_record(updated_hash, true)
}
