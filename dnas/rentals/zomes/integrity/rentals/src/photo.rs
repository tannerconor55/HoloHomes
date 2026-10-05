use hdi::prelude::*;

pub const MAX_PHOTO_URL_CHARS: usize = 2048;
pub const MAX_PHOTO_HASH_CHARS: usize = 128;
pub const MAX_PHOTO_CAPTION_CHARS: usize = 500;

#[hdk_entry_helper]
#[derive(Clone, PartialEq)]
pub struct ListingPhoto {
    /// The listing this photo belongs to.
    pub listing_hash: ActionHash,
    /// External/hybrid storage location for the image bytes.
    pub storage_url: String,
    /// Content hash supplied by the storage layer.
    pub content_hash: String,
    /// Optional human-readable caption.
    pub caption: String,
    /// Lower values appear first.
    pub sort_order: u32,
    /// Whether this image is the listing's cover photo.
    pub is_cover: bool,
}

pub fn validate_listing_photo(
    photo: &ListingPhoto,
) -> ExternResult<ValidateCallbackResult> {
    reject_if!(
        photo.storage_url.trim().is_empty(),
        "A photo needs a storage URL"
    );
    reject_if!(
        photo.storage_url.chars().count() > MAX_PHOTO_URL_CHARS,
        "Photo storage URLs are limited to {MAX_PHOTO_URL_CHARS} characters"
    );
    reject_if!(
        photo.content_hash.trim().is_empty(),
        "A photo needs a content hash"
    );
    reject_if!(
        photo.content_hash.chars().count() > MAX_PHOTO_HASH_CHARS,
        "Photo content hashes are limited to {MAX_PHOTO_HASH_CHARS} characters"
    );
    reject_if!(
        photo.caption.chars().count() > MAX_PHOTO_CAPTION_CHARS,
        "Photo captions are limited to {MAX_PHOTO_CAPTION_CHARS} characters"
    );

    Ok(ValidateCallbackResult::Valid)
}
