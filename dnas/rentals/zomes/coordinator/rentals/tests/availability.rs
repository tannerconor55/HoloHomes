use geo_utils::GeoPoint;
use holochain::prelude::*;
use holochain::sweettest::*;
use rentals::review::CreateReviewInput;
use rentals::search::{SearchListingsInput, SearchListingsOutput, SearchMethod};
use rentals_integrity::BookingDecision;

mod common;
use common::*;

const CENTER: GeoPoint = GeoPoint {
    lat: 38.7075,
    lng: -9.1364,
};

fn search(input: SearchInputOverrides) -> SearchListingsInput {
    SearchListingsInput {
        center: CENTER,
        radius_km: 5.0,
        method: SearchMethod::Both,
        limit: None,
        local_only: false,
        min_guests: input.min_guests,
        max_guests: input.max_guests,
        available_check_in: input.available_check_in,
        available_check_out: input.available_check_out,
    }
}

#[derive(Default)]
struct SearchInputOverrides {
    min_guests: Option<u32>,
    max_guests: Option<u32>,
    available_check_in: Option<Timestamp>,
    available_check_out: Option<Timestamp>,
}

fn titles(output: &SearchListingsOutput) -> Vec<String> {
    output.matches.iter().map(|m| m.listing.title.clone()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn search_filters_by_guest_capacity() {
    let (conductors, cells) = setup(2).await;
    let host = conductors.get(0).unwrap();
    let guest_zome = cells[1].zome("rentals");

    create_listing_with_guests(host, &cells[0], "Studio for two", 38.7110, -9.1390, 2).await;
    create_listing_with_guests(host, &cells[0], "Family house", 38.7139, -9.1300, 8).await;
    await_consistency(&cells).await.unwrap();

    let big_groups: SearchListingsOutput = conductors
        .get(1)
        .unwrap()
        .call(
            &guest_zome,
            "search_listings",
            search(SearchInputOverrides {
                min_guests: Some(6),
                ..Default::default()
            }),
        )
        .await;
    assert_eq!(titles(&big_groups), ["Family house"]);

    let small_only: SearchListingsOutput = conductors
        .get(1)
        .unwrap()
        .call(
            &guest_zome,
            "search_listings",
            search(SearchInputOverrides {
                max_guests: Some(3),
                ..Default::default()
            }),
        )
        .await;
    assert_eq!(titles(&small_only), ["Studio for two"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn search_filters_by_available_dates_and_reports_busy_ranges() {
    let (conductors, cells) = setup(2).await;
    let (host, guest) = (conductors.get(0).unwrap(), conductors.get(1).unwrap());
    let (host_zome, guest_zome) = (cells[0].zome("rentals"), cells[1].zome("rentals"));

    let listing = create_listing(host, &cells[0], "Baixa loft", 38.7110, -9.1390).await;
    await_consistency(&cells).await.unwrap();

    // A pending request should not block the dates.
    let pending = request_booking(guest, &cells[1], &listing, 10, 12).await;
    // An accepted one should.
    let accepted = request_booking(guest, &cells[1], &listing, 20, 25).await;
    await_consistency(&cells).await.unwrap();
    let _: Record = host
        .call(&host_zome, "respond_to_booking", response_input(&accepted, BookingDecision::Accepted))
        .await;
    await_consistency(&cells).await.unwrap();

    // Overlapping the accepted stay is excluded...
    let overlapping: SearchListingsOutput = guest
        .call(
            &guest_zome,
            "search_listings",
            search(SearchInputOverrides {
                available_check_in: Some(days_from_now(22)),
                available_check_out: Some(days_from_now(24)),
                ..Default::default()
            }),
        )
        .await;
    assert!(titles(&overlapping).is_empty());

    // ...but the pending request's dates are still searchable.
    let over_pending: SearchListingsOutput = guest
        .call(
            &guest_zome,
            "search_listings",
            search(SearchInputOverrides {
                available_check_in: Some(days_from_now(10)),
                available_check_out: Some(days_from_now(12)),
                ..Default::default()
            }),
        )
        .await;
    assert_eq!(titles(&over_pending), ["Baixa loft"]);

    // A window that clears the accepted stay entirely is fine.
    let clear: SearchListingsOutput = guest
        .call(
            &guest_zome,
            "search_listings",
            search(SearchInputOverrides {
                available_check_in: Some(days_from_now(1)),
                available_check_out: Some(days_from_now(3)),
                ..Default::default()
            }),
        )
        .await;
    assert_eq!(titles(&clear), ["Baixa loft"]);

    // Without a date filter, everything shows, with the accepted range reported so the UI
    // can grey it out — and the pending request does NOT appear as busy.
    let unfiltered: SearchListingsOutput = guest
        .call(&guest_zome, "search_listings", search(SearchInputOverrides::default()))
        .await;
    assert_eq!(unfiltered.matches.len(), 1);
    let busy = &unfiltered.matches[0].busy_ranges;
    assert_eq!(busy.len(), 1);
    // Compared against the request as actually stored, not a fresh `days_from_now(20)`:
    // `Timestamp::now()` moves between calls, so recomputing "now" here would not
    // reliably match the value baked into the booking many seconds ago.
    let accepted_booking = get_booking(guest, &guest_zome, &accepted).await;
    assert_eq!(busy[0].check_in, accepted_booking.request.check_in);
    assert_eq!(busy[0].check_out, accepted_booking.request.check_out);
    let _ = pending;
}

#[tokio::test(flavor = "multi_thread")]
async fn search_reports_rating() {
    let (conductors, cells) = setup(2).await;
    let (host, guest) = (conductors.get(0).unwrap(), conductors.get(1).unwrap());
    let (host_zome, guest_zome) = (cells[0].zome("rentals"), cells[1].zome("rentals"));

    let listing = create_listing(host, &cells[0], "Alfama flat", 38.7139, -9.1300).await;
    await_consistency(&cells).await.unwrap();

    let unrated: SearchListingsOutput = guest
        .call(&guest_zome, "search_listings", search(SearchInputOverrides::default()))
        .await;
    assert_eq!(unrated.matches[0].rating.review_count, 0);
    assert_eq!(unrated.matches[0].rating.average_rating, None);

    let stay = request_booking(guest, &cells[1], &listing, -4, -2).await;
    await_consistency(&cells).await.unwrap();
    let _: Record = host
        .call(&host_zome, "respond_to_booking", response_input(&stay, BookingDecision::Accepted))
        .await;
    await_consistency(&cells).await.unwrap();
    let _: Record = guest
        .call(
            &guest_zome,
            "create_review",
            CreateReviewInput {
                booking_request_hash: stay,
                rating: 4,
                text: "Nice".into(),
            },
        )
        .await;
    await_consistency(&cells).await.unwrap();

    let rated: SearchListingsOutput = guest
        .call(&guest_zome, "search_listings", search(SearchInputOverrides::default()))
        .await;
    assert_eq!(rated.matches[0].rating.review_count, 1);
    assert_eq!(rated.matches[0].rating.average_rating, Some(4.0));
}
