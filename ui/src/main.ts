import 'leaflet/dist/leaflet.css';
import './style.css';

import { decodeHashFromBase64, encodeHashToBase64, type ActionHash } from '@holochain/client';
import { cellToBoundary } from 'h3-js';
import L from 'leaflet';

import {
  Api,
  decodeEntry,
  type BookingView,
  type BusyRange,
  type GeoPoint,
  type Listing,
  type ListingMatch,
  type ListingView,
  type RatingOverview,
  type SearchMethod,
} from './api';
import { searchPlaces, reverseGeocode, type Place } from './geocode';
import { geohashBounds } from './geo';
import { signInWithSimulatedVault, signInWithVault, vaultStatusText } from './identity';

const DAY_MS = 86_400_000;

// ── Small helpers ───────────────────────────────────────────────────────

const $ = <T extends HTMLElement>(selector: string, root: ParentNode = document) =>
  root.querySelector(selector) as T;
const esc = (text: string) =>
  text.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!);
const b64 = (hash: Uint8Array) => encodeHashToBase64(hash);
const short = (hash: Uint8Array) => `${b64(hash).slice(0, 12)}…`;
const distance = (km: number) => (km < 1 ? `${Math.round(km * 1000)} m` : `${km.toFixed(2)} km`);
const isoDay = (offsetDays: number) => new Date(Date.now() + offsetDays * DAY_MS).toISOString().slice(0, 10);
const dayToMicros = (isoDate: string) => Date.parse(isoDate) * 1000;
const microsToDay = (micros: number) => new Date(micros / 1000).toISOString().slice(0, 10);
const stars = (rating: number) => '★'.repeat(rating) + '☆'.repeat(5 - rating);

function toast(message: string, isError = false) {
  const note = document.createElement('div');
  note.textContent = message;
  if (isError) note.className = 'error';
  $('#toast').append(note);
  setTimeout(() => note.remove(), isError ? 9000 : 3500);
}

/** Pulls the human-readable reason out of a zome error (`Guest("…")` or `Invalid("…")`). */
function errorMessage(error: unknown): string {
  const text = error instanceof Error ? error.message : JSON.stringify(error);
  return text.match(/(?:Guest|Invalid)\(\\?"(.+?)\\?"\)/)?.[1] ?? text;
}

/** Runs an action with its button disabled, reporting errors as a toast. */
async function busy(button: HTMLButtonElement | null, action: () => Promise<void>) {
  if (button) button.disabled = true;
  try {
    await action();
  } catch (error) {
    console.error(error);
    toast(errorMessage(error), true);
  } finally {
    if (button) button.disabled = false;
  }
}

// ── Layout ──────────────────────────────────────────────────────────────

$('#app').innerHTML = `
  <header>
    <h1>HoloAirBNB</h1>
    <span id="me" class="chip">Connecting…</span>
    <div id="identity"></div>
  </header>
  <div class="location-bar">
    <div class="place-search">
      <input id="place-query" type="text" placeholder="Search a postcode, city, or address…" autocomplete="off" />
      <ul id="place-results" hidden></ul>
    </div>
    <p class="hint">Click the map to fine-tune · <span id="picked">…</span></p>
  </div>
  <main>
    <section class="map-panel">
      <div id="map"></div>
    </section>
    <section class="side">
      <nav class="tabs">
        <button data-tab="explore" class="active">Explore</button>
        <button data-tab="host">Host</button>
        <button data-tab="trips">My trips</button>
      </nav>

      <div data-panel="explore">
        <form id="search-form" class="card">
          <div class="row" style="margin:0">
            <label style="flex:1">Radius: <output id="radius-out">3 km</output>
              <input name="radius" type="range" min="0.5" max="200" step="0.5" value="3" />
            </label>
            <div class="view-toggle">
              <button type="button" data-view="map" class="active">🗺️ Map</button>
              <button type="button" data-view="list">📋 List</button>
            </div>
          </div>
          <div class="grid2">
            <label>Index
              <select name="searchMethod">
                <option value="Both">Geohash + H3</option>
                <option value="Geohash">Geohash only</option>
                <option value="H3">H3 only</option>
              </select>
            </label>
            <label class="check"><input type="checkbox" name="cells" checked /> Draw index cells</label>
          </div>
          <details>
            <summary>Guests &amp; availability filters</summary>
            <div class="grid2">
              <label>Min guests <input name="minGuests" type="number" min="1" placeholder="Any" /></label>
              <label>Max guests <input name="maxGuests" type="number" min="1" placeholder="Any" /></label>
            </div>
            <div class="grid2">
              <label>Available from <input name="availFrom" type="date" /></label>
              <label>Available to <input name="availTo" type="date" /></label>
            </div>
            <p class="meta">Set both dates to hide listings already booked over those nights.</p>
          </details>
          <button>Search around the picked point</button>
        </form>
        <div id="search-meta"></div>
        <div id="results"></div>
      </div>

      <div data-panel="host" hidden>
        <form id="listing-form" class="card">
          <h3>New listing</h3>
          <p class="meta">Search your property's address, postcode, or city above — or click the map — to set where it is.</p>
          <label>Title <input name="listingTitle" required maxlength="120" placeholder="Sunny loft in Baixa" /></label>
          <label>Description <textarea name="description" maxlength="5000"></textarea></label>
          <label>Max guests <input name="guests" type="number" min="1" max="50" value="2" /></label>
          <p id="listing-preview" class="meta"></p>
          <button>Publish listing</button>
        </form>
        <div class="row"><h3>My listings</h3><button class="ghost" id="refresh-host">Refresh</button></div>
        <div id="my-listings"><p class="muted">Loading…</p></div>
      </div>

      <div data-panel="trips" hidden>
        <div class="row"><h3>My bookings</h3><button class="ghost" id="refresh-trips">Refresh</button></div>
        <div id="my-trips"><p class="muted">Loading…</p></div>
      </div>
    </section>
  </main>
  <div id="toast"></div>
`;

// ── Map ─────────────────────────────────────────────────────────────────

let picked: GeoPoint = { lat: 38.7075, lng: -9.1364 };
const map = L.map('map').setView([picked.lat, picked.lng], 13);
L.tileLayer('https://tile.openstreetmap.org/{z}/{x}/{y}.png', {
  maxZoom: 19,
  attribution: '&copy; OpenStreetMap contributors',
}).addTo(map);
const pickMarker = L.circleMarker([picked.lat, picked.lng], { radius: 7, color: '#1d1b18', fillOpacity: 1 }).addTo(map);
const searchLayer = L.layerGroup().addTo(map);

let api: Api;

map.on('click', (event: L.LeafletMouseEvent) => {
  pick({ lat: event.latlng.lat, lng: ((event.latlng.lng + 540) % 360) - 180 });
});

let reverseGeocodeAbort: AbortController | undefined;

function pick(point: GeoPoint, recenter = false) {
  picked = point;
  pickMarker.setLatLng([picked.lat, picked.lng]);
  if (recenter) map.setView([picked.lat, picked.lng], Math.max(map.getZoom(), 13));
  showPicked();
}

function showPicked() {
  const coords = `${picked.lat.toFixed(5)}, ${picked.lng.toFixed(5)}`;
  $('#picked').textContent = coords;

  reverseGeocodeAbort?.abort();
  const controller = (reverseGeocodeAbort = new AbortController());
  reverseGeocode(picked.lat, picked.lng, controller.signal).then((label) => {
    if (controller.signal.aborted) return;
    $('#picked').textContent = label ? `${label} (${coords})` : coords;
  });

  if (!api) return;
  api
    .encodeLocation(picked)
    .then(({ geohash, h3_cell }) => {
      $('#listing-preview').innerHTML = `Will be stored as geohash <code>${geohash}</code> · H3 <code>${h3_cell}</code>`;
    })
    .catch(() => {});
}

// ── Place search (postcode / city / address → coordinates) ─────────────

const placeQuery = $<HTMLInputElement>('#place-query');
const placeResults = $<HTMLUListElement>('#place-results');
let placeSearchAbort: AbortController | undefined;
let placeSearchTimer: ReturnType<typeof setTimeout> | undefined;

function closePlaceResults() {
  placeResults.hidden = true;
  placeResults.innerHTML = '';
}

placeQuery.addEventListener('input', () => {
  clearTimeout(placeSearchTimer);
  const query = placeQuery.value.trim();
  if (query.length < 3) {
    closePlaceResults();
    return;
  }
  placeSearchTimer = setTimeout(() => runPlaceSearch(query), 300);
});

placeQuery.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') closePlaceResults();
});

document.addEventListener('click', (event) => {
  if (!(event.target instanceof Node)) return;
  if (!placeResults.contains(event.target) && event.target !== placeQuery) closePlaceResults();
});

async function runPlaceSearch(query: string) {
  placeSearchAbort?.abort();
  const controller = (placeSearchAbort = new AbortController());
  try {
    const places = await searchPlaces(query, 6, controller.signal);
    if (controller.signal.aborted) return;
    renderPlaceResults(places);
  } catch (error) {
    if (controller.signal.aborted) return;
    placeResults.innerHTML = `<li class="place-error">${esc(errorMessage(error))}</li>`;
    placeResults.hidden = false;
  }
}

function renderPlaceResults(places: Place[]) {
  if (places.length === 0) {
    placeResults.innerHTML = '<li class="place-error">No matches</li>';
    placeResults.hidden = false;
    return;
  }
  placeResults.innerHTML = places.map((place, i) => `<li data-index="${i}">${esc(place.label)}</li>`).join('');
  placeResults.hidden = false;
  placeResults.querySelectorAll('li[data-index]').forEach((li, i) => {
    li.addEventListener('click', () => {
      const place = places[i];
      pick({ lat: place.lat, lng: place.lng }, true);
      placeQuery.value = place.label;
      closePlaceResults();
    });
  });
}

// ── Map / list view toggle ───────────────────────────────────────────────

let viewMode: 'map' | 'list' = 'map';

document.querySelectorAll<HTMLButtonElement>('.view-toggle button').forEach((button) =>
  button.addEventListener('click', () => {
    viewMode = button.dataset.view as 'map' | 'list';
    document.querySelectorAll('.view-toggle button').forEach((b) => b.classList.toggle('active', b === button));
    document.querySelector('main')!.classList.toggle('list-view', viewMode === 'list');
    setTimeout(() => map.invalidateSize(), 0);
  }),
);

// ── Tabs ────────────────────────────────────────────────────────────────

document.querySelectorAll<HTMLButtonElement>('.tabs button').forEach((tab) =>
  tab.addEventListener('click', () => {
    document.querySelectorAll('.tabs button').forEach((t) => t.classList.toggle('active', t === tab));
    document.querySelectorAll<HTMLElement>('[data-panel]').forEach((panel) => {
      panel.hidden = panel.dataset.panel !== tab.dataset.tab;
    });
    if (tab.dataset.tab === 'host') loadMyListings();
    if (tab.dataset.tab === 'trips') loadMyTrips();
    setTimeout(() => map.invalidateSize(), 0);
  }),
);

// ── Identity ────────────────────────────────────────────────────────────

async function renderIdentity() {
  const area = $('#identity');
  const links = await api.getIdentityLinks(api.myAgent);
  if (links.length > 0) {
    area.innerHTML = `
      <span class="chip ok" title="${b64(links[0].linked_agent)}">✓ Flowsta identity ${short(links[0].linked_agent)}</span>
      <button class="ghost" id="revoke">Unlink</button>`;
    $<HTMLButtonElement>('#revoke').onclick = (e) =>
      busy(e.currentTarget as HTMLButtonElement, async () => {
        await api.revokeLink(links[0].action_hash);
        toast('Identity unlinked');
        await renderIdentity();
      });
    return;
  }
  area.innerHTML = `
    <span class="chip" id="vault-status">Checking Flowsta Vault…</span>
    <button id="vault-signin">Sign in with Flowsta Vault</button>
    <button class="ghost" id="fake-signin" title="Uses a throwaway key instead of a real Vault">Simulate Vault (dev)</button>`;
  vaultStatusText().then((text) => ($('#vault-status').textContent = text));
  $<HTMLButtonElement>('#vault-signin').onclick = (e) =>
    busy(e.currentTarget as HTMLButtonElement, async () => {
      await signInWithVault(api);
      toast('Signed in with Flowsta Vault');
      await renderIdentity();
    });
  $<HTMLButtonElement>('#fake-signin').onclick = (e) =>
    busy(e.currentTarget as HTMLButtonElement, async () => {
      await signInWithSimulatedVault(api);
      toast('Linked a simulated Vault identity (development only)');
      await renderIdentity();
    });
}

// ── Explore: search, reviews, booking ───────────────────────────────────

const searchForm = $<HTMLFormElement>('#search-form');
searchForm.radius.addEventListener('input', () => {
  $('#radius-out').textContent = `${searchForm.radius.value} km`;
});

searchForm.addEventListener('submit', (event) => {
  event.preventDefault();
  const button = $<HTMLButtonElement>('button', searchForm);
  busy(button, async () => {
    const radius = Number(searchForm.radius.value);
    const method = searchForm.searchMethod.value as SearchMethod;
    const minGuests = searchForm.minGuests.value ? Number(searchForm.minGuests.value) : null;
    const maxGuests = searchForm.maxGuests.value ? Number(searchForm.maxGuests.value) : null;
    const availFrom = searchForm.availFrom.value || null;
    const availTo = searchForm.availTo.value || null;
    if ((availFrom && !availTo) || (!availFrom && availTo)) {
      throw new Error('Set both an "available from" and "available to" date, or neither');
    }
    const output = await api.searchListings({
      center: picked,
      radius_km: radius,
      method,
      min_guests: minGuests,
      max_guests: maxGuests,
      available_check_in: availFrom ? dayToMicros(availFrom) : null,
      available_check_out: availTo ? dayToMicros(availTo) : null,
    });
    drawSearch(output.matches, radius, searchForm.cells.checked ? output : null);
    $('#search-meta').innerHTML = `
      <p class="meta">
        ${output.matches.length} listing(s) within ${radius} km.
        ${output.geohash_precision !== null ? `Read ${output.geohash_cells.length} geohash cells (precision ${output.geohash_precision}).` : ''}
        ${output.h3_resolution !== null ? `Read ${output.h3_cells.length} H3 cells (resolution ${output.h3_resolution}).` : ''}
        ${output.coverage_complete ? '' : '<strong>Radius exceeds the index; some listings may be missing.</strong>'}
        <span class="legend"><span style="background:var(--geohash)"></span>geohash<span style="background:var(--h3)"></span>H3</span>
      </p>`;
    renderResults(output.matches);
  });
});

function drawSearch(matches: ListingMatch[], radiusKm: number, cells: { geohash_cells: string[]; h3_cells: string[] } | null) {
  searchLayer.clearLayers();
  const circle = L.circle([picked.lat, picked.lng], { radius: radiusKm * 1000, color: '#1d1b18', weight: 1, fill: false, dashArray: '4 4' });
  searchLayer.addLayer(circle);
  if (cells) {
    for (const cell of cells.geohash_cells) {
      searchLayer.addLayer(L.rectangle(geohashBounds(cell), { color: '#3867d6', weight: 1, fillOpacity: 0.04 }));
    }
    for (const cell of cells.h3_cells) {
      searchLayer.addLayer(L.polygon(cellToBoundary(cell), { color: '#d98b1f', weight: 1, fillOpacity: 0.04 }));
    }
  }
  for (const match of matches) {
    const { lat, lng } = match.listing.location;
    searchLayer.addLayer(
      L.circleMarker([lat, lng], { radius: 8, color: '#d6453d', fillOpacity: 0.9 }).bindPopup(esc(match.listing.title)),
    );
  }
  map.fitBounds(circle.getBounds(), { maxZoom: 16 });
}

/** Keyed by base64 listing hash, so the booking form can check dates against busy_ranges. */
const currentMatches = new Map<string, ListingMatch>();

function ratingLine(rating: RatingOverview) {
  if (rating.review_count === 0) return '<span class="muted">No reviews yet</span>';
  return `<span class="stars">${stars(Math.round(rating.average_rating ?? 0))}</span> ${rating.average_rating?.toFixed(1)} (${rating.review_count})`;
}

function busyRangeText(ranges: BusyRange[]) {
  if (ranges.length === 0) return 'No confirmed bookings yet';
  return `Booked: ${ranges.map((r) => `${microsToDay(r.check_in)} → ${microsToDay(r.check_out)}`).join(' · ')}`;
}

function isBookedRightNow(ranges: BusyRange[]) {
  const now = Date.now() * 1000;
  return ranges.some((r) => r.check_in <= now && now < r.check_out);
}

function overlapsBusy(checkIn: number, checkOut: number, ranges: BusyRange[]) {
  return ranges.some((r) => checkIn < r.check_out && r.check_in < checkOut);
}

function renderResults(matches: ListingMatch[]) {
  const results = $('#results');
  currentMatches.clear();
  if (matches.length === 0) {
    results.innerHTML = '<p class="muted">Nothing here yet. Publish a listing from the Host tab (in another agent window).</p>';
    return;
  }
  results.innerHTML = matches
    .map((match) => {
      const { listing } = match;
      const hash = b64(match.listing_hash);
      currentMatches.set(hash, match);
      const bookedNow = isBookedRightNow(match.busy_ranges);
      return `
        <article class="card${bookedNow ? ' currently-booked' : ''}" data-listing="${hash}">
          <div class="row" style="margin:0">
            <h3>${esc(listing.title)}</h3>
            ${bookedNow ? '<span class="chip">Currently booked</span>' : ''}
          </div>
          ${listing.description ? `<p>${esc(listing.description)}</p>` : ''}
          <p class="meta">${ratingLine(match.rating)}</p>
          <p class="meta">
            Haversine <strong>${distance(match.haversine_km)}</strong> · Euclidean ${distance(match.euclidean_km)}
            · up to ${listing.max_guests} guests<br />
            geohash <code>${listing.geohash}</code> · H3 <code>${listing.h3_cell}</code>
          </p>
          <p class="meta">${busyRangeText(match.busy_ranges)}</p>
          <details class="reviews"><summary>Reviews</summary><div class="reviews-body muted">Loading…</div></details>
          <form class="book">
            <div class="grid2">
              <label>Check-in <input name="checkIn" type="date" value="${isoDay(-3)}" required /></label>
              <label>Check-out <input name="checkOut" type="date" value="${isoDay(-1)}" required /></label>
            </div>
            <div class="grid2">
              <label>Guests <input name="guests" type="number" min="1" max="${listing.max_guests}" value="1" /></label>
              <label>Message <input name="message" placeholder="Hi!" /></label>
            </div>
            <p class="meta booking-warning" hidden></p>
            <p class="meta">Past dates are allowed, so you can try reviews straight away.</p>
            <button>Request booking</button>
          </form>
        </article>`;
    })
    .join('');
}

/** Warns (and blocks submit) when the chosen dates overlap a busy range already known
 * from the last search. The host's own accept step still enforces this for real — this
 * is just an earlier, friendlier heads-up. */
$('#results').addEventListener('input', (event) => {
  const form = (event.target as HTMLElement).closest<HTMLFormElement>('form.book');
  if (!form) return;
  const listingHash = form.closest<HTMLElement>('[data-listing]')!.dataset.listing!;
  const match = currentMatches.get(listingHash);
  const warning = $<HTMLElement>('.booking-warning', form);
  const submit = $<HTMLButtonElement>('button', form);
  if (!match || !form.checkIn.value || !form.checkOut.value) {
    warning.hidden = true;
    submit.disabled = false;
    return;
  }
  const overlap = overlapsBusy(dayToMicros(form.checkIn.value), dayToMicros(form.checkOut.value), match.busy_ranges);
  warning.hidden = !overlap;
  warning.textContent = overlap ? 'These dates overlap a confirmed booking — the host will likely decline.' : '';
  submit.disabled = overlap;
});

$('#results').addEventListener('submit', (event) => {
  const form = event.target as HTMLFormElement;
  if (!form.classList.contains('book')) return;
  event.preventDefault();
  const listingHash = form.closest<HTMLElement>('[data-listing]')!.dataset.listing!;
  const checkIn = dayToMicros(form.checkIn.value);
  const checkOut = dayToMicros(form.checkOut.value);
  const match = currentMatches.get(listingHash);
  if (match && overlapsBusy(checkIn, checkOut, match.busy_ranges)) {
    toast('These dates overlap a confirmed booking for this listing.', true);
    return;
  }
  busy($<HTMLButtonElement>('button', form), async () => {
    await api.requestBooking({
      listing_hash: decodeHashFromBase64(listingHash),
      check_in: checkIn,
      check_out: checkOut,
      guests: Number(form.guests.value),
      message: form.message.value,
    });
    toast('Booking requested. The host sees it under Host → My listings.');
  });
});

$('#results').addEventListener(
  'toggle',
  (event) => {
    const details = event.target as HTMLDetailsElement;
    if (!details.classList.contains('reviews') || !details.open) return;
    const listingHash = decodeHashFromBase64(details.closest<HTMLElement>('[data-listing]')!.dataset.listing!);
    renderReviews(listingHash, $('.reviews-body', details));
  },
  true,
);

async function renderReviews(listingHash: ActionHash, target: HTMLElement) {
  try {
    const { reviews, summary } = await api.getReviewsForListing(listingHash);
    if (summary.review_count === 0) {
      target.innerHTML = 'No reviews yet.';
      return;
    }
    target.classList.remove('muted');
    target.innerHTML = `
      <p class="meta"><span class="stars">${stars(Math.round(summary.average_rating ?? 0))}</span>
        ${summary.average_rating?.toFixed(1)} from ${summary.review_count} review(s), ${summary.verified_review_count} Flowsta-verified</p>
      ${reviews
        .map(
          (view) => `
          <div class="request">
            <span class="stars">${stars(view.review.rating)}</span>
            ${view.flowsta_identity ? `<span class="verified" title="${b64(view.flowsta_identity)}">✓ verified identity</span>` : ''}
            <p>${esc(view.review.text)}</p>
            <p class="meta">by ${short(view.reviewer)} on ${microsToDay(view.created_at)}</p>
          </div>`,
        )
        .join('')}`;
  } catch (error) {
    target.textContent = errorMessage(error);
  }
}

// ── Host: listings and booking requests ─────────────────────────────────

const listingForm = $<HTMLFormElement>('#listing-form');
listingForm.addEventListener('submit', (event) => {
  event.preventDefault();
  busy($<HTMLButtonElement>('button', listingForm), async () => {
    await api.createListing({
      title: listingForm.listingTitle.value,
      description: listingForm.description.value,
      location: picked,
      max_guests: Number(listingForm.guests.value),
    });
    listingForm.reset();
    toast('Listing published');
    await loadMyListings();
  });
});
$('#refresh-host').addEventListener('click', () => loadMyListings());

let myListings: ListingView[] = [];

async function loadMyListings() {
  const container = $('#my-listings');
  try {
    myListings = await api.getMyListings();
    if (myListings.length === 0) {
      container.innerHTML = '<p class="muted">No listings yet. Pick a point on the map and publish one.</p>';
      return;
    }
    const sections = await Promise.all(
      myListings.map(async (view, index) => {
        const requests = await api.getBookingRequestsForListing(view.listing_hash);
        const archived = view.listing.status === 'Archived';
        return `
          <article class="card" data-index="${index}">
            <div class="row" style="margin:0">
              <h3>${esc(view.listing.title)} ${archived ? '<span class="chip">archived</span>' : ''}</h3>
              <button class="ghost" data-action="toggle-status">${archived ? 'Reactivate' : 'Archive'}</button>
            </div>
            <p class="meta">${view.listing.location.lat.toFixed(4)}, ${view.listing.location.lng.toFixed(4)} · up to ${view.listing.max_guests} guests</p>
            ${requests.length === 0 ? '<p class="meta">No booking requests yet.</p>' : requests.map(renderRequest).join('')}
          </article>`;
      }),
    );
    container.innerHTML = sections.join('');
  } catch (error) {
    container.innerHTML = `<p class="muted">${esc(errorMessage(error))}</p>`;
  }
}

function bookingStatus(view: BookingView) {
  const status = view.response?.decision ?? 'Pending';
  return `<span class="status-${status}">${status}</span>`;
}

function renderRequest(view: BookingView) {
  const guest = view.request_record.signed_action.hashed.content.header.author;
  return `
    <div class="request" data-request="${b64(view.request_hash)}">
      <p class="meta">${bookingStatus(view)} · ${microsToDay(view.request.check_in)} → ${microsToDay(view.request.check_out)}
        · ${view.request.guests} guest(s) · from ${short(guest)}</p>
      ${view.request.message ? `<p>“${esc(view.request.message)}”</p>` : ''}
      ${view.response ? '' : `<button data-action="Accepted">Accept</button> <button class="ghost" data-action="Declined">Decline</button>`}
    </div>`;
}

$('#my-listings').addEventListener('click', (event) => {
  const button = (event.target as HTMLElement).closest<HTMLButtonElement>('button[data-action]');
  if (!button) return;
  const action = button.dataset.action!;
  if (action === 'toggle-status') {
    const view = myListings[Number(button.closest<HTMLElement>('[data-index]')!.dataset.index)];
    busy(button, async () => {
      await api.updateListing({
        original_listing_hash: view.listing_hash,
        previous_listing_hash: view.record.signed_action.hashed.hash,
        title: view.listing.title,
        description: view.listing.description,
        max_guests: view.listing.max_guests,
        status: view.listing.status === 'Active' ? 'Archived' : 'Active',
      });
      await loadMyListings();
    });
    return;
  }
  const requestHash = decodeHashFromBase64(button.closest<HTMLElement>('[data-request]')!.dataset.request!);
  busy(button, async () => {
    await api.respondToBooking({ request_hash: requestHash, decision: action as 'Accepted' | 'Declined', note: '' });
    toast(`Booking ${action.toLowerCase()}`);
    await loadMyListings();
  });
});

// ── Trips: my bookings and reviews ──────────────────────────────────────

$('#refresh-trips').addEventListener('click', () => loadMyTrips());

async function loadMyTrips() {
  const container = $('#my-trips');
  try {
    const bookings = await api.getMyBookings();
    if (bookings.length === 0) {
      container.innerHTML = '<p class="muted">No bookings yet. Search on the Explore tab and request one.</p>';
      return;
    }
    const cards = await Promise.all(
      bookings.map(async (view) => {
        const listingRecord = await api.getListing(view.request.listing_hash);
        const title = listingRecord ? decodeEntry<Listing>(listingRecord).title : 'Listing not synced yet';
        const canReview = view.response?.decision === 'Accepted';
        return `
          <article class="card" data-request="${b64(view.request_hash)}">
            <h3>${esc(title)}</h3>
            <p class="meta">${bookingStatus(view)} · ${microsToDay(view.request.check_in)} → ${microsToDay(view.request.check_out)} · ${view.request.guests} guest(s)</p>
            ${
              canReview
                ? `<form class="review">
                    <div class="grid2">
                      <label>Rating
                        <select name="rating">${[5, 4, 3, 2, 1].map((n) => `<option value="${n}">${stars(n)}</option>`).join('')}</select>
                      </label>
                    </div>
                    <label>Review <textarea name="text" maxlength="2000" placeholder="How was your stay?"></textarea></label>
                    <button>Post review</button>
                  </form>`
                : ''
            }
          </article>`;
      }),
    );
    container.innerHTML = cards.join('');
  } catch (error) {
    container.innerHTML = `<p class="muted">${esc(errorMessage(error))}</p>`;
  }
}

$('#my-trips').addEventListener('submit', (event) => {
  const form = event.target as HTMLFormElement;
  if (!form.classList.contains('review')) return;
  event.preventDefault();
  const requestHash = decodeHashFromBase64(form.closest<HTMLElement>('[data-request]')!.dataset.request!);
  busy($<HTMLButtonElement>('button', form), async () => {
    await api.createReview({ booking_request_hash: requestHash, rating: Number(form.rating.value), text: form.text.value });
    form.replaceWith(Object.assign(document.createElement('p'), { className: 'meta', textContent: 'Review posted ✓' }));
    toast('Review posted');
  });
});

// ── Start ───────────────────────────────────────────────────────────────

async function start() {
  showPicked();
  try {
    api = await Api.connect();
  } catch (error) {
    $('#me').textContent = 'Not connected to Holochain';
    toast(`Could not connect to the conductor: ${errorMessage(error)}. Launch with "npm start".`, true);
    return;
  }
  $('#me').textContent = `You: ${short(api.myAgent)}`;
  $('#me').title = b64(api.myAgent);
  showPicked();
  await renderIdentity().catch((error) => toast(errorMessage(error), true));
}

start();
