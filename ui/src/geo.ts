const BASE32 = '0123456789bcdefghjkmnpqrstuvwxyz';

/** South-west and north-east corners of a geohash cell, as [lat, lng] pairs. */
export function geohashBounds(hash: string): [[number, number], [number, number]] {
  let [latMin, latMax, lngMin, lngMax] = [-90, 90, -180, 180];
  let isLng = true;
  for (const char of hash) {
    const value = BASE32.indexOf(char);
    for (let bit = 4; bit >= 0; bit--) {
      const on = (value >> bit) & 1;
      if (isLng) {
        const mid = (lngMin + lngMax) / 2;
        if (on) lngMin = mid;
        else lngMax = mid;
      } else {
        const mid = (latMin + latMax) / 2;
        if (on) latMin = mid;
        else latMax = mid;
      }
      isLng = !isLng;
    }
  }
  return [
    [latMin, lngMin],
    [latMax, lngMax],
  ];
}
