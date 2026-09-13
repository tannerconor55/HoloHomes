use geo_utils::{geohash_index_cells, h3_index_cells, CellIndex, GeoPoint};
use hdi::prelude::*;
use holochain_serialized_bytes::UnsafeBytes;

use crate::{invalid, must_get_listing, LinkTypes};

/// The path a geohash cell's listings hang off, e.g. `geohash.eyckr`.
pub fn geohash_bucket_path(cell: &str) -> Path {
    Path::from(format!("geohash.{cell}"))
}

/// The path an H3 cell's listings hang off, e.g. `h3.85393233fffffff`.
pub fn h3_bucket_path(cell: CellIndex) -> Path {
    Path::from(format!("h3.{cell}"))
}

/// Carried on every geo index link, so a search can rank candidates by distance
/// without fetching each listing.
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone, PartialEq)]
pub struct GeoIndexTag {
    pub location: GeoPoint,
}

impl GeoIndexTag {
    pub fn to_link_tag(&self) -> Result<LinkTag, SerializedBytesError> {
        let bytes = SerializedBytes::try_from(self.clone())?;
        Ok(LinkTag::new(bytes.bytes().clone()))
    }

    pub fn from_link_tag(tag: &LinkTag) -> Result<Self, SerializedBytesError> {
        Self::try_from(SerializedBytes::from(UnsafeBytes::from(tag.0.clone())))
    }
}

/// A geo index link is valid only if the host authored it, it targets the listing's create
/// action, its tag carries the listing's real location, and its base is one of the index cells
/// for that location. Without this anyone could make a listing appear somewhere else.
pub(crate) fn validate_geo_index_link(
    link_type: &LinkTypes,
    action: &TypedAction<CreateLinkData>,
    target: &ActionHash,
) -> ExternResult<ValidateCallbackResult> {
    let (record, listing) = check!(must_get_listing(target)?);
    reject_if!(
        record.action().author() != action.author(),
        "Only the host can add their listing to the geo index"
    );
    reject_if!(
        !matches!(record.action().data, ActionData::Create(_)),
        "Geo index links must target the listing's original create action"
    );
    let tag = check!(GeoIndexTag::from_link_tag(&action.tag)
        .map_err(|e| format!("Malformed geo index tag: {e:?}")));
    reject_if!(
        tag.location != listing.location,
        "The geo index tag must carry the listing's location"
    );

    let bucket_paths: Vec<Path> = match link_type {
        LinkTypes::GeohashToListing => {
            check!(geohash_index_cells(listing.location).map_err(|e| e.to_string()))
                .iter()
                .map(|cell| geohash_bucket_path(cell))
                .collect()
        }
        LinkTypes::H3CellToListing => {
            check!(h3_index_cells(listing.location).map_err(|e| e.to_string()))
                .into_iter()
                .map(h3_bucket_path)
                .collect()
        }
        _ => return invalid("Not a geo index link type"),
    };
    for path in bucket_paths {
        if AnyLinkableHash::from(path.path_entry_hash()?) == action.base_address {
            return Ok(ValidateCallbackResult::Valid);
        }
    }
    invalid("A geo index link must start from one of the listing's own index cells")
}
