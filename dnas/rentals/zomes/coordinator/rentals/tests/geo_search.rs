use geo_utils::GeoPoint;
use holochain::prelude::*;
use holochain::sweettest::*;
use rentals::listing::UpdateListingInput;
use rentals::search::{SearchListingsInput, SearchListingsOutput, SearchMethod};
use rentals_integrity::ListingStatus;

mod common;
use common::*;

/// Praça do Comércio, Lisbon.
const SEARCH_CENTER: GeoPoint = GeoPoint {
    lat: 38.7075,
    lng: -9.1364,
};

fn search(radius_km: f64, method: SearchMethod) -> SearchListingsInput {
    SearchListingsInput {
        center: SEARCH_CENTER,
        radius_km,
        method,
        limit: None,
        local_only: false,
        min_guests: None,
        max_guests: None,
        available_check_in: None,
        available_check_out: None,
    }
}

fn titles(output: &SearchListingsOutput) -> Vec<String> {
    output
        .matches
        .iter()
        .map(|m| m.listing.title.clone())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn guests_find_nearby_listings_by_geohash_and_h3() {
    let (conductors, cells) = setup(2).await;
    let host = conductors.get(0).unwrap();
    let guest = conductors.get(1).unwrap();
    let guest_zome = cells[1].zome("rentals");

    // Roughly 0.45 km, 0.9 km, 6.2 km and 275 km from the search centre.
    let baixa = create_listing(host, &cells[0], "Baixa loft", 38.7110, -9.1390).await;
    create_listing(host, &cells[0], "Alfama flat", 38.7139, -9.1300).await;
    create_listing(host, &cells[0], "Belem house", 38.6970, -9.2060).await;
    create_listing(host, &cells[0], "Porto apartment", 41.1496, -8.6110).await;
    await_consistency(&cells).await.unwrap();

    for method in [SearchMethod::Geohash, SearchMethod::H3, SearchMethod::Both] {
        let nearby: SearchListingsOutput = guest
            .call(&guest_zome, "search_listings", search(3.0, method))
            .await;
        assert!(nearby.coverage_complete, "{method:?}");
        assert_eq!(titles(&nearby), ["Baixa loft", "Alfama flat"], "{method:?}");
        for m in &nearby.matches {
            assert!(m.haversine_km <= 3.0);
            let relative_gap = (m.euclidean_km - m.haversine_km).abs() / m.haversine_km;
            assert!(relative_gap < 0.01, "{method:?}: {m:?}");
        }

        let wider: SearchListingsOutput = guest
            .call(&guest_zome, "search_listings", search(10.0, method))
            .await;
        assert_eq!(
            titles(&wider),
            ["Baixa loft", "Alfama flat", "Belem house"],
            "{method:?}"
        );
    }

    // Archived listings drop out of search.
    let _: Record = host
        .call(
            &cells[0].zome("rentals"),
            "update_listing",
            UpdateListingInput {
                original_listing_hash: baixa.clone(),
                previous_listing_hash: baixa,
                title: "Baixa loft".into(),
                description: "No longer available".into(),
                max_guests: 4,
                status: ListingStatus::Archived,
            },
        )
        .await;
    await_consistency(&cells).await.unwrap();
    let after_archive: SearchListingsOutput = guest
        .call(&guest_zome, "search_listings", search(3.0, SearchMethod::Both))
        .await;
    assert_eq!(titles(&after_archive), ["Alfama flat"]);
}
