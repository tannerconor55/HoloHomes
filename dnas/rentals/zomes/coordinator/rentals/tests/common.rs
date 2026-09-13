#![allow(dead_code)]

use std::path::Path;

use geo_utils::GeoPoint;
use holochain::prelude::*;
use holochain::sweettest::*;
use rentals::booking::{BookingView, RequestBookingInput, RespondToBookingInput};
use rentals::listing::CreateListingInput;
use rentals_integrity::BookingDecision;

pub const DAY_MICROS: i64 = 86_400_000_000;

/// One conductor per agent, each running the packed rentals DNA.
pub async fn setup(agents: usize) -> (SweetConductorBatch, Vec<SweetCell>) {
    let mut conductors = SweetConductorBatch::standard(agents).await;
    let dna_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../workdir/rentals.dna");
    let dna_file = SweetDnaFile::from_bundle(&dna_path).await.unwrap();
    let apps = conductors
        .setup_app("holo-airbnb", &[dna_file])
        .await
        .unwrap();
    let cells = apps.cells_flattened();
    (conductors, cells)
}

pub fn days_from_now(days: i64) -> Timestamp {
    Timestamp::from_micros(Timestamp::now().as_micros() + days * DAY_MICROS)
}

pub async fn create_listing(
    conductor: &SweetConductor,
    cell: &SweetCell,
    title: &str,
    lat: f64,
    lng: f64,
) -> ActionHash {
    create_listing_with_guests(conductor, cell, title, lat, lng, 4).await
}

pub async fn create_listing_with_guests(
    conductor: &SweetConductor,
    cell: &SweetCell,
    title: &str,
    lat: f64,
    lng: f64,
    max_guests: u32,
) -> ActionHash {
    let record: Record = conductor
        .call(
            &cell.zome("rentals"),
            "create_listing",
            CreateListingInput {
                title: title.into(),
                description: format!("{title}, a test listing"),
                location: GeoPoint { lat, lng },
                max_guests,
            },
        )
        .await;
    record.action_address().clone()
}

pub fn booking_input(listing_hash: &ActionHash, check_in_days: i64, check_out_days: i64) -> RequestBookingInput {
    RequestBookingInput {
        listing_hash: listing_hash.clone(),
        check_in: days_from_now(check_in_days),
        check_out: days_from_now(check_out_days),
        guests: 2,
        message: "Hello!".into(),
    }
}

pub async fn request_booking(
    conductor: &SweetConductor,
    cell: &SweetCell,
    listing_hash: &ActionHash,
    check_in_days: i64,
    check_out_days: i64,
) -> ActionHash {
    let record: Record = conductor
        .call(
            &cell.zome("rentals"),
            "request_booking",
            booking_input(listing_hash, check_in_days, check_out_days),
        )
        .await;
    record.action_address().clone()
}

pub async fn get_booking(
    conductor: &SweetConductor,
    zome: &SweetZome,
    request_hash: &ActionHash,
) -> BookingView {
    conductor
        .call::<_, Option<BookingView>>(zome, "get_booking", request_hash.clone())
        .await
        .expect("booking not found")
}

pub fn response_input(request_hash: &ActionHash, decision: BookingDecision) -> RespondToBookingInput {
    RespondToBookingInput {
        request_hash: request_hash.clone(),
        decision,
        note: String::new(),
    }
}

/// Asserts a fallible zome call failed with an error mentioning `expected`.
pub fn assert_error_contains<T: std::fmt::Debug>(
    result: holochain::conductor::api::error::ConductorApiResult<T>,
    expected: &str,
) {
    let error = format!("{:?}", result.expect_err("expected the zome call to fail"));
    assert!(
        error.contains(expected),
        "expected an error containing {expected:?}, got: {error}"
    );
}
