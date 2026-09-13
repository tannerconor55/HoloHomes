/**
 * Turns free text (a postcode, city, or address) into coordinates, and coordinates back
 * into a readable place name, via Photon (https://photon.komoot.io) — a free OpenStreetMap
 * geocoder that, unlike Nominatim, sends `Access-Control-Allow-Origin: *`, so it can be
 * called straight from the browser with no backend proxy.
 *
 * This is only ever a convenience for picking a `GeoPoint`. Nothing here is stored on the
 * DHT — listings only ever carry lat/lng, never an address string.
 *
 * The public instance is rate-limited and meant for exactly this kind of light, interactive
 * use (https://photon.komoot.io/); a production deployment should run its own instance or
 * use a paid provider instead.
 */

const PHOTON_URL = 'https://photon.komoot.io';

type PhotonProperties = {
  name?: string;
  street?: string;
  city?: string;
  county?: string;
  state?: string;
  country?: string;
  postcode?: string;
};

type PhotonFeature = {
  properties: PhotonProperties;
  geometry: { coordinates: [number, number] }; // [lng, lat]
};

type PhotonResponse = { features: PhotonFeature[] };

export type Place = { label: string; lat: number; lng: number };

function formatLabel(p: PhotonProperties): string {
  const parts = [p.name, p.street, p.city ?? p.county, p.state, p.postcode, p.country].filter(
    (part): part is string => !!part,
  );
  // A city-level result often repeats its name as the city too; keep only the first of any duplicate.
  return [...new Set(parts)].join(', ');
}

function toPlace(feature: PhotonFeature): Place {
  const [lng, lat] = feature.geometry.coordinates;
  return { label: formatLabel(feature.properties) || `${lat.toFixed(5)}, ${lng.toFixed(5)}`, lat, lng };
}

/** Looks up a postcode, city name, or address. Empty on no match; throws on a network error. */
export async function searchPlaces(query: string, limit = 5, signal?: AbortSignal): Promise<Place[]> {
  const url = `${PHOTON_URL}/api/?q=${encodeURIComponent(query)}&limit=${limit}`;
  const response = await fetch(url, { signal });
  if (!response.ok) throw new Error(`Geocoding search failed (${response.status})`);
  const data = (await response.json()) as PhotonResponse;
  return data.features.map(toPlace);
}

/** The place name at a point, or null if Photon has nothing there or the request fails. */
export async function reverseGeocode(lat: number, lng: number, signal?: AbortSignal): Promise<string | null> {
  try {
    const url = `${PHOTON_URL}/reverse?lat=${lat}&lon=${lng}`;
    const response = await fetch(url, { signal });
    if (!response.ok) return null;
    const data = (await response.json()) as PhotonResponse;
    return data.features[0] ? formatLabel(data.features[0].properties) : null;
  } catch {
    return null;
  }
}
