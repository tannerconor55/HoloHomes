//! Geospatial helpers for HoloAirBNB: geohash and H3 encoding, search-cell planning,
//! and Haversine / Euclidean distances.
//!
//! The `rentals` integrity zome calls these from `validate()`, so everything here must stay
//! pure and deterministic. Floating-point results are reproducible because every validator
//! runs the same compiled wasm. Changing this crate changes the DNA hash.

use std::fmt;

use h3o::LatLng;
pub use h3o::{CellIndex, Resolution};
use serde::{Deserialize, Serialize};

/// Mean Earth radius (IUGG), in kilometres.
pub const EARTH_RADIUS_KM: f64 = 6371.0088;

/// Geohash precision stored on each listing (cells of roughly 5 m).
pub const LISTING_GEOHASH_PRECISION: usize = 9;

/// H3 resolution stored on each listing (hexagon edges of roughly 175 m).
pub const LISTING_H3_RESOLUTION: Resolution = Resolution::Nine;

/// Geohash lengths each listing is indexed under, coarse to fine.
/// Cells at the equator: 3 ≈ 156 km, 4 ≈ 39 × 20 km, 5 ≈ 4.9 km, 6 ≈ 1.2 × 0.6 km.
pub const GEOHASH_INDEX_PRECISIONS: [usize; 4] = [3, 4, 5, 6];

/// H3 resolutions each listing is indexed under, coarse to fine
/// (average hexagon edges ≈ 60 km, 8.5 km and 1.2 km).
pub const H3_INDEX_RESOLUTIONS: [Resolution; 3] =
    [Resolution::Three, Resolution::Five, Resolution::Seven];

/// Largest `grid_disk` ring a search reads. Ring 3 is 37 cells, so 37 link lookups.
pub const MAX_H3_RING: u32 = 3;

/// Largest search radius accepted, in kilometres.
pub const MAX_SEARCH_RADIUS_KM: f64 = 500.0;

/// H3 hexagons are not all the same size, so rings are widened by this factor.
const H3_RING_SAFETY: f64 = 1.25;

/// A WGS84 coordinate in decimal degrees.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct GeoPoint {
    pub lat: f64,
    pub lng: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GeoError {
    InvalidLatitude(f64),
    InvalidLongitude(f64),
    InvalidRadius(f64),
    Geohash(String),
    H3(String),
}

impl fmt::Display for GeoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GeoError::InvalidLatitude(lat) => write!(f, "latitude {lat} is not within -90..=90"),
            GeoError::InvalidLongitude(lng) => {
                write!(f, "longitude {lng} is not within -180..=180")
            }
            GeoError::InvalidRadius(radius) => write!(
                f,
                "search radius {radius} km must be above 0 and at most {MAX_SEARCH_RADIUS_KM} km"
            ),
            GeoError::Geohash(e) => write!(f, "geohash error: {e}"),
            GeoError::H3(e) => write!(f, "H3 error: {e}"),
        }
    }
}

impl std::error::Error for GeoError {}

impl GeoPoint {
    pub fn new(lat: f64, lng: f64) -> Result<Self, GeoError> {
        let point = Self { lat, lng };
        point.check()?;
        Ok(point)
    }

    /// Rejects NaN, infinities and out-of-range degrees.
    pub fn check(&self) -> Result<(), GeoError> {
        if !(-90.0..=90.0).contains(&self.lat) {
            return Err(GeoError::InvalidLatitude(self.lat));
        }
        if !(-180.0..=180.0).contains(&self.lng) {
            return Err(GeoError::InvalidLongitude(self.lng));
        }
        Ok(())
    }
}

// ── Encoding ────────────────────────────────────────────────────────────

pub fn geohash(point: GeoPoint, precision: usize) -> Result<String, GeoError> {
    point.check()?;
    geohash::encode(
        geohash::Coord {
            x: point.lng,
            y: point.lat,
        },
        precision,
    )
    .map_err(|e| GeoError::Geohash(e.to_string()))
}

/// A geohash cell plus its eight neighbours, deduplicated (neighbours coincide near the poles).
pub fn geohash_neighborhood(cell: &str) -> Result<Vec<String>, GeoError> {
    let n = geohash::neighbors(cell).map_err(|e| GeoError::Geohash(e.to_string()))?;
    let mut cells = vec![cell.to_string(), n.n, n.ne, n.e, n.se, n.s, n.sw, n.w, n.nw];
    cells.sort();
    cells.dedup();
    Ok(cells)
}

pub fn h3_cell(point: GeoPoint, resolution: Resolution) -> Result<CellIndex, GeoError> {
    point.check()?;
    let latlng = LatLng::new(point.lat, point.lng).map_err(|e| GeoError::H3(format!("{e:?}")))?;
    Ok(latlng.to_cell(resolution))
}

/// The geohash cells a listing at `point` is indexed under, one per `GEOHASH_INDEX_PRECISIONS`.
pub fn geohash_index_cells(point: GeoPoint) -> Result<Vec<String>, GeoError> {
    GEOHASH_INDEX_PRECISIONS
        .iter()
        .map(|&precision| geohash(point, precision))
        .collect()
}

/// The H3 cells a listing at `point` is indexed under, one per `H3_INDEX_RESOLUTIONS`.
pub fn h3_index_cells(point: GeoPoint) -> Result<Vec<CellIndex>, GeoError> {
    H3_INDEX_RESOLUTIONS
        .iter()
        .map(|&resolution| h3_cell(point, resolution))
        .collect()
}

// ── Distances ───────────────────────────────────────────────────────────

/// Great-circle distance on a spherical Earth. Accurate to about 0.5% anywhere on the globe.
pub fn haversine_km(a: GeoPoint, b: GeoPoint) -> f64 {
    let (phi1, phi2) = (a.lat.to_radians(), b.lat.to_radians());
    let d_phi = (b.lat - a.lat).to_radians();
    let d_lambda = (b.lng - a.lng).to_radians();
    let h = (d_phi / 2.0).sin().powi(2) + phi1.cos() * phi2.cos() * (d_lambda / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * h.sqrt().min(1.0).asin()
}

/// Straight-line distance on an equirectangular projection centred on the two points'
/// mean latitude.
///
/// Raw Euclidean distance over latitude/longitude degrees is meaningless away from the
/// equator, so longitude is scaled by `cos(latitude)` first. Cheaper than Haversine and
/// within a fraction of a percent of it at city scale; it drifts over hundreds of
/// kilometres and near the poles.
pub fn euclidean_km(a: GeoPoint, b: GeoPoint) -> f64 {
    let mean_lat = ((a.lat + b.lat) / 2.0).to_radians();
    let mut d_lng = b.lng - a.lng;
    // Take the short way across the antimeridian.
    if d_lng > 180.0 {
        d_lng -= 360.0;
    } else if d_lng < -180.0 {
        d_lng += 360.0;
    }
    let x = d_lng.to_radians() * mean_lat.cos();
    let y = (b.lat - a.lat).to_radians();
    EARTH_RADIUS_KM * (x * x + y * y).sqrt()
}

// ── Search planning ─────────────────────────────────────────────────────

/// Which geohash cells to read to find every listing within a radius.
#[derive(Debug, Clone, PartialEq)]
pub struct GeohashSearchPlan {
    pub precision: usize,
    pub cells: Vec<String>,
    /// False when even the coarsest indexed precision cannot guarantee the 3×3
    /// neighbourhood covers the radius; some matches may then be missed.
    pub complete: bool,
}

/// Which H3 cells to read to find every listing within a radius.
#[derive(Debug, Clone, PartialEq)]
pub struct H3SearchPlan {
    pub resolution: Resolution,
    pub k: u32,
    pub cells: Vec<CellIndex>,
    /// False when the coarsest indexed resolution would need more than `MAX_H3_RING` rings.
    pub complete: bool,
}

fn check_radius(radius_km: f64) -> Result<(), GeoError> {
    if radius_km > 0.0 && radius_km <= MAX_SEARCH_RADIUS_KM {
        Ok(())
    } else {
        Err(GeoError::InvalidRadius(radius_km))
    }
}

/// Picks the finest indexed geohash precision whose 3×3 neighbourhood around `center`
/// covers a circle of `radius_km`. Finer cells mean fewer false candidates per lookup.
pub fn geohash_search_plan(center: GeoPoint, radius_km: f64) -> Result<GeohashSearchPlan, GeoError> {
    check_radius(radius_km)?;
    for &precision in GEOHASH_INDEX_PRECISIONS.iter().rev() {
        let cell = geohash(center, precision)?;
        if geohash_min_span_km(&cell)? >= radius_km {
            return Ok(GeohashSearchPlan {
                precision,
                cells: geohash_neighborhood(&cell)?,
                complete: true,
            });
        }
    }
    let precision = GEOHASH_INDEX_PRECISIONS[0];
    let cell = geohash(center, precision)?;
    Ok(GeohashSearchPlan {
        precision,
        cells: geohash_neighborhood(&cell)?,
        complete: false,
    })
}

/// The narrowest extent of a geohash cell, in km.
///
/// Any point within this distance of the cell lies in its 3×3 neighbourhood. East-west
/// extent shrinks with latitude, so it is measured at the poleward edge of the neighbour row.
fn geohash_min_span_km(cell: &str) -> Result<f64, GeoError> {
    let bbox = geohash::decode_bbox(cell).map_err(|e| GeoError::Geohash(e.to_string()))?;
    let (min, max) = (bbox.min(), bbox.max());
    let height_deg = max.y - min.y;
    let width_deg = max.x - min.x;
    let poleward_lat = (min.y.abs().max(max.y.abs()) + height_deg).min(90.0);
    let width_km = width_deg.to_radians() * EARTH_RADIUS_KM * poleward_lat.to_radians().cos();
    let height_km = height_deg.to_radians() * EARTH_RADIUS_KM;
    Ok(width_km.min(height_km))
}

/// Picks the finest indexed H3 resolution that covers `radius_km` within `MAX_H3_RING` rings.
pub fn h3_search_plan(center: GeoPoint, radius_km: f64) -> Result<H3SearchPlan, GeoError> {
    check_radius(radius_km)?;
    for &resolution in H3_INDEX_RESOLUTIONS.iter().rev() {
        let k = h3_ring_for_radius(radius_km, resolution);
        if k <= MAX_H3_RING {
            return Ok(H3SearchPlan {
                resolution,
                k,
                cells: h3_cell(center, resolution)?.grid_disk(k),
                complete: true,
            });
        }
    }
    let resolution = H3_INDEX_RESOLUTIONS[0];
    Ok(H3SearchPlan {
        resolution,
        k: MAX_H3_RING,
        cells: h3_cell(center, resolution)?.grid_disk(MAX_H3_RING),
        complete: false,
    })
}

/// The `grid_disk` ring count that includes every cell holding a point within `radius_km`.
///
/// With `e` the edge length (the hexagon circumradius), a point within `r` of the query sits
/// in a cell whose centre is within `r + 2e` of the query cell's centre. Cells outside ring
/// `k` have centres at least `(k + 1) · 1.5e` away, so `k = ⌊(r + 2e) / 1.5e⌋` suffices.
/// The result is widened by `H3_RING_SAFETY` because real cell sizes vary around the average.
pub fn h3_ring_for_radius(radius_km: f64, resolution: Resolution) -> u32 {
    let edge = resolution.edge_length_km();
    ((radius_km + 2.0 * edge) / (1.5 * edge) * H3_RING_SAFETY).floor() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(lat: f64, lng: f64) -> GeoPoint {
        GeoPoint::new(lat, lng).unwrap()
    }

    /// The point `distance_km` from `origin` along the initial bearing `bearing_deg`.
    fn destination(origin: GeoPoint, bearing_deg: f64, distance_km: f64) -> GeoPoint {
        let delta = distance_km / EARTH_RADIUS_KM;
        let theta = bearing_deg.to_radians();
        let phi1 = origin.lat.to_radians();
        let phi2 = (phi1.sin() * delta.cos() + phi1.cos() * delta.sin() * theta.cos()).asin();
        let lambda2 = origin.lng.to_radians()
            + (theta.sin() * delta.sin() * phi1.cos()).atan2(delta.cos() - phi1.sin() * phi2.sin());
        let lng = (lambda2.to_degrees() + 540.0) % 360.0 - 180.0;
        point(phi2.to_degrees(), lng)
    }

    #[test]
    fn geohash_matches_reference_value() {
        assert_eq!(geohash(point(57.64911, 10.40744), 11).unwrap(), "u4pruydqqvj");
    }

    #[test]
    fn h3_matches_reference_value() {
        let cell = h3_cell(point(37.3615593, -122.0553238), Resolution::Seven).unwrap();
        assert_eq!(cell.to_string(), "87283472bffffff");
    }

    #[test]
    fn haversine_paris_to_london() {
        let d = haversine_km(point(48.8566, 2.3522), point(51.5074, -0.1278));
        assert!((d - 343.5).abs() < 1.0, "got {d}");
    }

    #[test]
    fn euclidean_tracks_haversine_at_city_scale() {
        let a = point(38.7075, -9.1364);
        for bearing in [0.0, 45.0, 90.0, 200.0, 310.0] {
            let b = destination(a, bearing, 5.0);
            let (h, e) = (haversine_km(a, b), euclidean_km(a, b));
            assert!((h - 5.0).abs() < 1e-6, "haversine {h}");
            assert!((e - h).abs() / h < 0.005, "euclidean {e} vs haversine {h}");
        }
    }

    #[test]
    fn euclidean_takes_short_way_across_antimeridian() {
        let (a, b) = (point(0.0, 179.9), point(0.0, -179.9));
        let (h, e) = (haversine_km(a, b), euclidean_km(a, b));
        assert!(e < 25.0, "got {e}");
        assert!((e - h).abs() < 0.01);
    }

    #[test]
    fn rejects_invalid_input() {
        assert!(GeoPoint::new(91.0, 0.0).is_err());
        assert!(GeoPoint::new(0.0, 180.5).is_err());
        assert!(GeoPoint::new(f64::NAN, 0.0).is_err());
        assert!(geohash_search_plan(point(0.0, 0.0), 0.0).is_err());
        assert!(h3_search_plan(point(0.0, 0.0), MAX_SEARCH_RADIUS_KM + 1.0).is_err());
    }

    #[test]
    fn index_cells_are_computed_from_the_point_at_each_level() {
        let p = point(38.7110, -9.1390);
        // Geohashes nest: every coarser cell is a prefix of the listing's own geohash.
        let full = geohash(p, LISTING_GEOHASH_PRECISION).unwrap();
        for cell in geohash_index_cells(p).unwrap() {
            assert!(full.starts_with(&cell));
        }
        // H3 cells do not nest exactly, so a point's coarse cell can differ from its fine
        // cell's parent. Index cells must come straight from the point, as searches do.
        let cells = h3_index_cells(p).unwrap();
        for (cell, resolution) in cells.iter().zip(H3_INDEX_RESOLUTIONS) {
            assert_eq!(*cell, h3_cell(p, resolution).unwrap());
        }
    }

    /// The core guarantee of both search plans: no point inside the radius is missed.
    #[test]
    fn search_plans_cover_every_point_within_radius() {
        let centers = [
            point(0.0, 0.0),
            point(38.7075, -9.1364),
            point(59.9139, 10.7522),
            point(-33.8688, 151.2093),
            point(64.1466, -21.9426),
            point(0.5, 179.95),
        ];
        for center in centers {
            for radius in [0.25, 1.0, 3.0, 12.0, 40.0, 150.0] {
                let gh = geohash_search_plan(center, radius).unwrap();
                let h3 = h3_search_plan(center, radius).unwrap();
                if radius <= 40.0 {
                    assert!(gh.complete && h3.complete, "{center:?} r={radius}");
                }
                for step in 0..72 {
                    for fraction in [0.05, 0.5, 0.999] {
                        let p = destination(center, step as f64 * 5.0, radius * fraction);
                        if gh.complete {
                            let cell = geohash(p, gh.precision).unwrap();
                            assert!(gh.cells.contains(&cell), "geohash missed {p:?} ({center:?} r={radius})");
                        }
                        if h3.complete {
                            let cell = h3_cell(p, h3.resolution).unwrap();
                            assert!(h3.cells.contains(&cell), "H3 missed {p:?} ({center:?} r={radius})");
                        }
                    }
                }
            }
        }
    }
}
