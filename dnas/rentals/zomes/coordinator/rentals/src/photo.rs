use hdk::prelude::*;
use rentals_integrity::*;

use crate::{decode_entry, must_get_record};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AddListingPhotoInput {
    pub listing_hash: ActionHash,
    pub storage_url: String,
    pub content_hash: String,
    pub caption: String,
    pub sort_order: u32,
    pub is_cover: bool,
}

/// Adds photo metadata to a listing.
///
/// The image bytes are stored outside the DHT. This entry records the
/// storage location and content hash so clients can retrieve and verify it.
#[hdk_extern]
pub fn add_listing_photo(input: AddListingPhotoInput) -> ExternResult<Record> {
    let listing_record = must_get_record(input.listing_hash.clone(), true)?;
    let listing: Listing = decode_entry(&listing_record)?;

    if listing.status == ListingStatus::Archived {
        return Err(wasm_error!(WasmErrorInner::Guest(
            "Cannot add photos to an archived listing".into()
        )));
    }

    if listing_record.action().author() != &agent_info()?.agent_initial_pubkey {
        return Err(wasm_error!(WasmErrorInner::Guest(
            "Only the listing host can add photos".into()
        )));
    }

    let photo = ListingPhoto {
        listing_hash: input.listing_hash.clone(),
        storage_url: input.storage_url,
        content_hash: input.content_hash,
        caption: input.caption,
        sort_order: input.sort_order,
        is_cover: input.is_cover,
    };

    let photo_hash = create_entry(&EntryTypes::ListingPhoto(photo))?;

    create_link(
        input.listing_hash,
        photo_hash.clone(),
        LinkTypes::ListingToPhotos,
        (),
    )?;

    must_get_record(photo_hash, true)
}

#[hdk_extern]
pub fn get_listing_photos(listing_hash: ActionHash) -> ExternResult<Vec<ListingPhoto>> {
    let links = get_links(
        LinkQuery::try_new(listing_hash, LinkTypes::ListingToPhotos)?,
        GetStrategy::Network,
    )?;

    let mut photos = Vec::new();

    for link in links {
        let Some(photo_hash) = link.target.into_action_hash() else {
            continue;
        };

        let Some(record) = get(photo_hash, GetOptions::default())? else {
            continue;
        };

        if let Ok(photo) = decode_entry::<ListingPhoto>(&record) {
            photos.push(photo);
        }
    }

    photos.sort_by_key(|photo| (photo.sort_order, !photo.is_cover));

    Ok(photos)
}
