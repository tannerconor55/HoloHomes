use holochain::prelude::*;
use holochain::sweettest::*;
use rentals::review::{CreateReviewInput, ListingReviews};
use rentals_integrity::BookingDecision;

mod common;
use common::*;

fn review_input(request_hash: &ActionHash, rating: u8) -> CreateReviewInput {
    CreateReviewInput {
        booking_request_hash: request_hash.clone(),
        rating,
        text: "Lovely stay".into(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn guest_books_host_accepts_and_guest_reviews() {
    let (conductors, cells) = setup(3).await;
    let (host, guest, other) = (
        conductors.get(0).unwrap(),
        conductors.get(1).unwrap(),
        conductors.get(2).unwrap(),
    );
    let (host_zome, guest_zome, other_zome) = (
        cells[0].zome("rentals"),
        cells[1].zome("rentals"),
        cells[2].zome("rentals"),
    );

    let listing = create_listing(host, &cells[0], "Baixa loft", 38.7110, -9.1390).await;
    await_consistency(&cells).await.unwrap();

    // A stay that ended yesterday.
    let stay = request_booking(guest, &cells[1], &listing, -3, -1).await;
    // Another guest wants overlapping dates.
    let overlapping = request_booking(other, &cells[2], &listing, -2, 0).await;
    await_consistency(&cells).await.unwrap();

    // Only the host can answer a request.
    assert_error_contains(
        other
            .call_fallible::<_, Record>(
                &other_zome,
                "respond_to_booking",
                response_input(&stay, BookingDecision::Accepted),
            )
            .await,
        "Only the listing's host",
    );

    let _: Record = host
        .call(
            &host_zome,
            "respond_to_booking",
            response_input(&stay, BookingDecision::Accepted),
        )
        .await;
    assert_error_contains(
        host.call_fallible::<_, Record>(
            &host_zome,
            "respond_to_booking",
            response_input(&overlapping, BookingDecision::Accepted),
        )
        .await,
        "overlap",
    );
    await_consistency(&cells).await.unwrap();

    // Someone who did not make the booking cannot review it.
    assert_error_contains(
        other
            .call_fallible::<_, Record>(&other_zome, "create_review", review_input(&stay, 1))
            .await,
        "only review your own bookings",
    );

    let _: Record = guest
        .call(&guest_zome, "create_review", review_input(&stay, 5))
        .await;
    await_consistency(&cells).await.unwrap();
    assert_error_contains(
        guest
            .call_fallible::<_, Record>(&guest_zome, "create_review", review_input(&stay, 4))
            .await,
        "already reviewed",
    );

    let reviews: ListingReviews = host
        .call(&host_zome, "get_reviews_for_listing", listing)
        .await;
    assert_eq!(reviews.summary.review_count, 1);
    assert_eq!(reviews.summary.average_rating, Some(5.0));
    assert_eq!(reviews.summary.verified_review_count, 0);
    assert_eq!(&reviews.reviews[0].reviewer, cells[1].agent_pubkey());
}

#[tokio::test(flavor = "multi_thread")]
async fn reviews_are_rejected_by_validation_before_check_out() {
    let (conductors, cells) = setup(2).await;
    let (host, guest) = (conductors.get(0).unwrap(), conductors.get(1).unwrap());

    let listing = create_listing(host, &cells[0], "Alfama flat", 38.7139, -9.1300).await;
    await_consistency(&cells).await.unwrap();
    let future_stay = request_booking(guest, &cells[1], &listing, 1, 3).await;
    await_consistency(&cells).await.unwrap();
    let _: Record = host
        .call(
            &cells[0].zome("rentals"),
            "respond_to_booking",
            response_input(&future_stay, BookingDecision::Accepted),
        )
        .await;
    await_consistency(&cells).await.unwrap();

    // The coordinator does not check dates, so this rejection comes from integrity validation.
    assert_error_contains(
        guest
            .call_fallible::<_, Record>(
                &cells[1].zome("rentals"),
                "create_review",
                review_input(&future_stay, 5),
            )
            .await,
        "after check-out",
    );
}
