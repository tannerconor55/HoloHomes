use std::collections::BTreeMap;

use geo_utils::{
    euclidean_km, geohash, geohash_search_plan, h3_cell, h3_search_plan, haversine_km, GeoPoint,
    LISTING_GEOHASH_PRECISION, LISTING_H3_RESOLUTION,
};
use hdk::prelude::*;
use rentals_integrity::*;

use crate::booking::{busy_ranges_for_listing, ranges_overlap, BusyRange};
use crate::listing::{geo_error, latest_listing_record};
use crate::review::{rating_overview, RatingOverview};
use crate::{decode_entry, link_strategy};

pub const DEFAULT_SEARCH_LIMIT: u32 = 50;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMethod {
    /// Read the 3×3 geohash neighbourhood around the search point.
    Geohash,
    /// Read an H3 `grid_disk` around the search point.
    H3,
    /// Read both and merge the candidates.
    Both,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SearchListingsInput {
    pub center: GeoPoint,
    pub radius_km: f64,
    pub method: SearchMethod,
    /// Maximum listings to return, nearest first. Defaults to `DEFAULT_SEARCH_LIMIT`.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Only read what this node already holds. Faster, and the only option on a
    /// zero-arc (mobile) node that cannot wait on the network.
    #[serde(default)]
    pub local_only: bool,
    /// Only listings that host at least this many guests.
    #[serde(default)]
    pub min_guests: Option<u32>,
    /// Only listings that host at most this many guests.
    #[serde(default)]
    pub max_guests: Option<u32>,
    /// Both or neither: only listings with no accepted booking overlapping this stay.
    #[serde(default)]
    pub available_check_in: Option<Timestamp>,
    #[serde(default)]
    pub available_check_out: Option<Timestamp>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ListingMatch {
    /// The listing's original create action: pass this to `request_booking`.
    pub listing_hash: ActionHash,
    /// The latest version of the listing.
    pub record: Record,
    pub listing: Listing,
    pub haversine_km: f64,
    pub euclidean_km: f64,
    pub rating: RatingOverview,
    /// This listing's accepted booking date ranges, so a guest can see what's already
    /// taken without a separate call. Empty if nothing has been accepted yet.
    pub busy_ranges: Vec<BusyRange>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SearchListingsOutput {
    pub matches: Vec<ListingMatch>,
    pub geohash_precision: Option<usize>,
    pub geohash_cells: Vec<String>,
    pub h3_resolution: Option<u8>,
    pub h3_cells: Vec<String>,
    /// False when no lookup could cover the whole radius, so matches may be missing.
    pub coverage_complete: bool,
}

/// Finds active listings within `radius_km` of `center`, nearest first by Haversine distance.
///
/// Candidates come from geo index links, whose tags carry each listing's location, so
/// everything outside the radius is dropped before any listing record is fetched.
#[hdk_extern]
pub fn search_listings(input: SearchListingsInput) -> ExternResult<SearchListingsOutput> {
    input.center.check().map_err(geo_error)?;
    let mut candidates: BTreeMap<ActionHash, GeoPoint> = BTreeMap::new();
    let mut output = SearchListingsOutput {
        matches: Vec::new(),
        geohash_precision: None,
        geohash_cells: Vec::new(),
        h3_resolution: None,
        h3_cells: Vec::new(),
        coverage_complete: false,
    };

    if matches!(input.method, SearchMethod::Geohash | SearchMethod::Both) {
        let plan = geohash_search_plan(input.center, input.radius_km).map_err(geo_error)?;
        for cell in &plan.cells {
            collect_candidates(
                geohash_bucket_path(cell),
                LinkTypes::GeohashToListing,
                input.local_only,
                &mut candidates,
            )?;
        }
        output.geohash_precision = Some(plan.precision);
        output.geohash_cells = plan.cells;
        output.coverage_complete |= plan.complete;
    }
    if matches!(input.method, SearchMethod::H3 | SearchMethod::Both) {
        let plan = h3_search_plan(input.center, input.radius_km).map_err(geo_error)?;
        for cell in &plan.cells {
            collect_candidates(
                h3_bucket_path(*cell),
                LinkTypes::H3CellToListing,
                input.local_only,
                &mut candidates,
            )?;
        }
        output.h3_resolution = Some(u8::from(plan.resolution));
        output.h3_cells = plan.cells.iter().map(|cell| cell.to_string()).collect();
        output.coverage_complete |= plan.complete;
    }

    let mut nearby: Vec<(ActionHash, GeoPoint, f64)> = candidates
        .into_iter()
        .map(|(hash, location)| (hash, location, haversine_km(input.center, location)))
        .filter(|(_, _, distance)| *distance <= input.radius_km)
        .collect();
    nearby.sort_by(|a, b| a.2.total_cmp(&b.2).then_with(|| a.0.cmp(&b.0)));

    let limit = input.limit.unwrap_or(DEFAULT_SEARCH_LIMIT) as usize;
    for (listing_hash, location, distance) in nearby {
        if output.matches.len() >= limit {
            break;
        }
        // Links can arrive before the listing itself has reached this node.
        let Some(record) = latest_listing_record(listing_hash.clone(), input.local_only)? else {
            continue;
        };
        let listing: Listing = decode_entry(&record)?;
        if listing.status != ListingStatus::Active {
            continue;
        }
        if input.min_guests.is_some_and(|min| listing.max_guests < min)
            || input.max_guests.is_some_and(|max| listing.max_guests > max)
        {
            continue;
        }

        let busy_ranges = busy_ranges_for_listing(&listing_hash)?;
        if let (Some(check_in), Some(check_out)) =
            (input.available_check_in, input.available_check_out)
        {
            let unavailable = busy_ranges
                .iter()
                .any(|r| ranges_overlap(r.check_in, r.check_out, check_in, check_out));
            if unavailable {
                continue;
            }
        }

        output.matches.push(ListingMatch {
            rating: rating_overview(&listing_hash)?,
            listing_hash,
            record,
            listing,
            haversine_km: distance,
            euclidean_km: euclidean_km(input.center, location),
            busy_ranges,
        });
    }
    Ok(output)
}

fn collect_candidates(
    bucket: Path,
    link_type: LinkTypes,
    local_only: bool,
    candidates: &mut BTreeMap<ActionHash, GeoPoint>,
) -> ExternResult<()> {
    let links = get_links(
        LinkQuery::try_new(bucket.path_entry_hash()?, link_type)?,
        link_strategy(local_only),
    )?;
    for link in links {
        let Some(listing_hash) = link.target.into_action_hash() else {
            continue;
        };
        let Ok(tag) = GeoIndexTag::from_link_tag(&link.tag) else {
            continue;
        };
        candidates.insert(listing_hash, tag.location);
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DistanceInput {
    pub from: GeoPoint,
    pub to: GeoPoint,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DistanceOutput {
    pub haversine_km: f64,
    pub euclidean_km: f64,
}

#[hdk_extern]
pub fn distance_between(input: DistanceInput) -> ExternResult<DistanceOutput> {
    input.from.check().map_err(geo_error)?;
    input.to.check().map_err(geo_error)?;
    Ok(DistanceOutput {
        haversine_km: haversine_km(input.from, input.to),
        euclidean_km: euclidean_km(input.from, input.to),
    })
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LocationEncoding {
    pub geohash: String,
    pub h3_cell: String,
}

/// The geohash and H3 cell a listing at `location` would get, for previews in the UI.
#[hdk_extern]
pub fn encode_location(location: GeoPoint) -> ExternResult<LocationEncoding> {
    Ok(LocationEncoding {
        geohash: geohash(location, LISTING_GEOHASH_PRECISION).map_err(geo_error)?,
        h3_cell: h3_cell(location, LISTING_H3_RESOLUTION)
            .map_err(geo_error)?
            .to_string(),
    })
}
