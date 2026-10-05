import {
  AppWebsocket,
  type ActionHash,
  type AgentPubKey,
  type Record as HcRecord,
} from '@holochain/client';
import { decode } from '@msgpack/msgpack';

const ROLE = 'rentals';

type HolochainLauncherEnv = {
  APP_INTERFACE_PORT: number;
  INSTALLED_APP_ID: string;
  APP_INTERFACE_TOKEN: number[];
};

declare global {
  interface Window {
    __HC_LAUNCHER_ENV__?: HolochainLauncherEnv;
  }
}

// Mirrors of the Rust types in dnas/rentals/zomes. Keep them in step with the zome code.

export type GeoPoint = { lat: number; lng: number };
export type ListingStatus = 'Active' | 'Archived';
export type SearchMethod = 'Geohash' | 'H3' | 'Both';
export type BookingDecision = 'Accepted' | 'Declined';

export type Listing = {
  title: string;
  description: string;
  location: GeoPoint;
  geohash: string;
  h3_cell: string;
  max_guests: number;
  status: ListingStatus;
};

export type ListingPhoto = {
  listing_hash: ActionHash;
  storage_url: string;
  content_hash: string;
  caption: string;
  sort_order: number;
  is_cover: boolean;
};

export type ListingView = { listing_hash: ActionHash; record: HcRecord; listing: Listing };

export type RatingOverview = { review_count: number; average_rating: number | null };

/** A booking someone else already had accepted. Carries no guest identity. */
export type BusyRange = { check_in: number; check_out: number };

export type ListingMatch = ListingView & {
  haversine_km: number;
  euclidean_km: number;
  rating: RatingOverview;
  busy_ranges: BusyRange[];
};

export type SearchOutput = {
  matches: ListingMatch[];
  geohash_precision: number | null;
  geohash_cells: string[];
  h3_resolution: number | null;
  h3_cells: string[];
  coverage_complete: boolean;
};

export type SearchFilters = {
  min_guests?: number | null;
  max_guests?: number | null;
  available_check_in?: number | null;
  available_check_out?: number | null;
};

/** Timestamps are microseconds since the Unix epoch. */
export type BookingRequest = {
  listing_hash: ActionHash;
  check_in: number;
  check_out: number;
  guests: number;
  message: string;
};

export type BookingResponse = { request_hash: ActionHash; decision: BookingDecision; note: string };

export type BookingView = {
  request_hash: ActionHash;
  request_record: HcRecord;
  request: BookingRequest;
  response_hash: ActionHash | null;
  response: BookingResponse | null;
};

export type Review = {
  booking_request_hash: ActionHash;
  booking_response_hash: ActionHash;
  rating: number;
  text: string;
  identity_link_hash: ActionHash | null;
};

export type ReviewView = {
  review_hash: ActionHash;
  review: Review;
  reviewer: AgentPubKey;
  created_at: number;
  flowsta_identity: AgentPubKey | null;
};

export type ListingReviews = {
  reviews: ReviewView[];
  summary: { review_count: number; average_rating: number | null; verified_review_count: number };
};

export type IdentityLink = { action_hash: ActionHash; linked_agent: AgentPubKey };

export function decodeEntry<T>(record: HcRecord): T {
  const entry = record.entry as { Present?: { entry: Uint8Array } };
  if (!entry.Present) throw new Error('Record has no entry');
  return decode(entry.Present.entry) as T;
}

export class Api {
  private constructor(private readonly client: AppWebsocket) {}

  static async connect(): Promise<Api> {
    const launcher = window.__HC_LAUNCHER_ENV__;

    if (!launcher) {
      throw new Error(
        'HoloHomes must be launched by hc-spin so the Holochain app interface is available.',
      );
    }

    const client = await AppWebsocket.connect({
      url: new URL(`ws://localhost:${launcher.APP_INTERFACE_PORT}`),
      token: launcher.APP_INTERFACE_TOKEN,
    });

    return new Api(client);
  }

  get myAgent(): AgentPubKey {
    return this.client.myPubKey;
  }

  private call<T>(zome: 'rentals' | 'agent_linking', fn: string, payload: unknown = null): Promise<T> {
    return this.client.callZome({ role_name: ROLE, zome_name: zome, fn_name: fn, payload }) as Promise<T>;
  }

  // Listings and search
  createListing(input: { title: string; description: string; location: GeoPoint; max_guests: number }) {
    return this.call<HcRecord>('rentals', 'create_listing', input);
  }
  updateListing(input: {
    original_listing_hash: ActionHash;
    previous_listing_hash: ActionHash;
    title: string;
    description: string;
    max_guests: number;
    status: ListingStatus;
  }) {
    return this.call<HcRecord>('rentals', 'update_listing', input);
  }
  getListing(listingHash: ActionHash) {
    return this.call<HcRecord | null>('rentals', 'get_listing', listingHash);
  }
  getMyListings() {
    return this.call<ListingView[]>('rentals', 'get_my_listings');
  }
  addListingPhoto(input: {
    listing_hash: ActionHash;
    storage_url: string;
    content_hash: string;
    caption: string;
    sort_order: number;
    is_cover: boolean;
  }) {
    return this.call<HcRecord>('rentals', 'add_listing_photo', input);
  }
  getListingPhotos(listingHash: ActionHash) {
    return this.call<ListingPhoto[]>('rentals', 'get_listing_photos', listingHash);
  }
  searchListings(input: { center: GeoPoint; radius_km: number; method: SearchMethod } & SearchFilters) {
    return this.call<SearchOutput>('rentals', 'search_listings', {
      limit: null,
      local_only: false,
      min_guests: null,
      max_guests: null,
      available_check_in: null,
      available_check_out: null,
      ...input,
    });
  }
  encodeLocation(location: GeoPoint) {
    return this.call<{ geohash: string; h3_cell: string }>('rentals', 'encode_location', location);
  }
  getBusyRangesForListing(listingHash: ActionHash) {
    return this.call<BusyRange[]>('rentals', 'get_busy_ranges_for_listing', listingHash);
  }

  // Bookings
  requestBooking(input: BookingRequest) {
    return this.call<HcRecord>('rentals', 'request_booking', input);
  }
  respondToBooking(input: { request_hash: ActionHash; decision: BookingDecision; note: string }) {
    return this.call<HcRecord>('rentals', 'respond_to_booking', input);
  }
  getBookingRequestsForListing(listingHash: ActionHash) {
    return this.call<BookingView[]>('rentals', 'get_booking_requests_for_listing', listingHash);
  }
  getMyBookings() {
    return this.call<BookingView[]>('rentals', 'get_my_bookings');
  }

  // Reviews
  createReview(input: { booking_request_hash: ActionHash; rating: number; text: string }) {
    return this.call<HcRecord>('rentals', 'create_review', input);
  }
  getReviewsForListing(listingHash: ActionHash) {
    return this.call<ListingReviews>('rentals', 'get_reviews_for_listing', listingHash);
  }

  // Identity (Flowsta agent linking)
  createExternalLink(externalAgent: AgentPubKey, externalSignature: Uint8Array) {
    return this.call<ActionHash>('agent_linking', 'create_external_link', {
      external_agent: externalAgent,
      external_signature: externalSignature,
    });
  }
  getIdentityLinks(agent: AgentPubKey) {
    return this.call<IdentityLink[]>('agent_linking', 'get_identity_links', agent);
  }
  revokeLink(attestationHash: ActionHash) {
    return this.call<ActionHash>('agent_linking', 'revoke_link', attestationHash);
  }
}
