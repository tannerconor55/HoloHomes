# HoloAirBNB

A peer-to-peer rental booking hApp on **Holochain 0.7** (hdk 0.7.0 / hdi 0.8.0).

- Hosts publish listings. Each one gets a **geohash** and an **Uber H3** cell, and is indexed
  at several precisions of each.
- Guests search by radius using a geohash neighbourhood, an H3 `grid_disk`, or both. Results
  are ranked by **Haversine** distance, and **Euclidean** distance is returned alongside.
- Guests request bookings, hosts accept or decline, and guests review stays once they've ended.
- Sign-in is through **Flowsta Vault**, the same identity system ProofPoll uses. A signed-in
  guest's reviews carry a verifiable identity attestation.

Payments are out of scope for now.

## Layout

```
crates/
  geo_utils/              geohash, H3, Haversine, Euclidean, search-cell planning (pure Rust)
  identity_attestation/   Flowsta-compatible IsSamePerson attestation + signature checks
dnas/rentals/
  workdir/dna.yaml
  zomes/integrity/rentals/        Listing, BookingRequest, BookingResponse, Review + validation
  zomes/integrity/agent_linking/  IsSamePerson entry + validation
  zomes/coordinator/rentals/      listings, search, bookings, reviews (+ Sweettests in tests/)
  zomes/coordinator/agent_linking/ create_external_link, get_linked_agents, revoke_link
workdir/happ.yaml
```

## Commands

```bash
nix develop                 # Holonix main-0.7: hc, holochain, rust, node 24
npm run build:happ          # build zomes to wasm and pack workdir/holo_airbnb.happ
npm run test:geo            # geo unit tests (fast, no conductor)
npm test                    # build, pack, then run everything including Sweettests
```

The first Sweettest build compiles the `holochain` crate and takes around 10 minutes.
On a machine with little free RAM, cap the parallel jobs: `CARGO_BUILD_JOBS=4 npm test`.

## Location search

Every listing stores its exact `location`, its geohash at precision 9 and its H3 cell at
resolution 9. Validation recomputes both, so a host can't file a listing under the wrong place.

It is also linked from index cells at several levels, and each link tag carries the location:

| Index | Levels | Path |
|---|---|---|
| Geohash | precision 3, 4, 5, 6 | `geohash.<cell>` |
| H3 | resolution 3, 5, 7 | `h3.<cell>` |

Validation also checks those links: they must come from the host, start at one of the
listing's own cells, and carry its real location.

`search_listings { center, radius_km, method: Geohash | H3 | Both }` works like this:

1. **Plan cells.**
   - **Geohash:** use the finest precision whose cell is at least as narrow as the radius, then
     read that cell and its 8 neighbours. Cell width is measured at the poleward edge, since
     longitude cells narrow with latitude.
   - **H3:** use the finest resolution needing at most 3 rings, where
     `k = ⌊1.25 · (r + 2e) / 1.5e⌋` and `e` is the edge length.
2. **Read links.** Tags give each candidate's location without fetching the listing.
3. **Filter and sort.** Keep candidates within the Haversine radius, nearest first.
4. **Fetch.** Get the latest version of each match and drop archived listings.

The output reports `coverage_complete: false` when the radius is too big for the coarsest
index level, since matches may then be missing. That limit is about 167 km for H3 anywhere. For
geohash it's 156 km at the equator, shrinking to about 118 km in Lisbon and 74 km at 60°N, so
`Both` stays complete up to about 167 km.
A unit test checks both plans against thousands of points around six locations: the equator,
Lisbon, Oslo, Sydney, Reykjavik, and the antimeridian.

**Distances:**
- `haversine_km` is great-circle distance on a sphere of radius 6371.0088 km.
- `euclidean_km` is straight-line distance on an equirectangular projection (longitude scaled by
  `cos(mean latitude)`). Raw Euclidean distance over degrees is meaningless away from the
  equator. The projected version is within about 0.5% of Haversine at city scale, and drifts over
  hundreds of km and near the poles.

`distance_between` and `encode_location` are exposed for UI previews.

## Bookings and reviews

What the network enforces, because it's in integrity validation:
- A booking request points at a listing's original create action. The guest isn't the host, the
  stay is 1–365 nights, and there are 1–50 guests.
- Only the listing's host can author a `BookingResponse`. Responses and reviews can't be edited
  or deleted.
- A review must be written by the guest who made the booking, for a booking the host accepted.
  Its action timestamp must fall after check-out, and the rating must be 1–5.
- If a review carries `identity_link_hash`, that attestation must include the reviewer and both
  of its signatures must verify.
- A listing's location can never change.

What the coordinator checks on a best-effort basis:
- **Double booking.** When a host accepts, the coordinator checks the host's own source chain
  for overlapping accepted bookings. Only the host can author valid responses, so this local read
  is complete for a single device.
- **One review per booking.** Two devices racing could both pass this check, because validation
  can't prove that no other review exists.

## Identity and sign-in (Flowsta Vault)

ProofPoll's sign-in comes from [Flowsta Vault](https://github.com/WeAreFlowsta/flowsta-vault-app),
a desktop key manager, via the [`@flowsta/holochain`](https://github.com/WeAreFlowsta/flowsta-sdk)
SDK. ProofPoll pins Holochain 0.6 and depends on a `flowsta-agent-linking` crate that isn't
publicly available. So this repo ports the public, Apache-2.0 `agent_linking` zome from
[flowsta-identity-dna](https://github.com/WeAreFlowsta/flowsta-identity-dna) to Holochain 0.7,
keeping it wire-compatible:
- the same zome name and entry shape
- the same 78-byte payload: both raw 39-byte agent keys, sorted
- the same `create_external_link` input

The Vault signs that payload **raw** (checked against its `/link-identity` handler), so
verification uses `verify_signature_raw`.

UI flow:

```ts
import { linkFlowstaIdentity, getVaultStatus } from '@flowsta/holochain';
import { decodeHashFromBase64, encodeHashToBase64 } from '@holochain/client';

const { payload } = await linkFlowstaIdentity({
  appName: 'HoloAirBNB',
  clientId: import.meta.env.VITE_FLOWSTA_CLIENT_ID,   // register at dev.flowsta.com
  localAgentPubKey: encodeHashToBase64(myAgentPubKey),
});

await client.callZome({
  role_name: 'rentals',
  zome_name: 'agent_linking',
  fn_name: 'create_external_link',
  payload: {
    external_agent: decodeHashFromBase64(payload.vaultAgentPubKey),
    external_signature: Uint8Array.from(atob(payload.vaultSignature), c => c.charCodeAt(0)),
  },
});
```

`create_review` attaches the guest's newest active attestation automatically.
`get_reviews_for_listing` reports each reviewer's `flowsta_identity` and a
`verified_review_count`. Revoking the attestation removes the badge.

**What this proves, and what it doesn't:**
- **It proves** that the reviewer's agent key and a Flowsta identity were controlled by the same
  person when they linked. It also gives one identity across devices, which raises the cost of
  sock-puppet reviews.
- **It doesn't prove** legal identity. It isn't KYC or a government ID check. That would need a
  verifiable-credential issuer layered on top.

Signing documents (for example, a rental agreement PDF) is available through the SDK's
`signDocument()`. It records a signature over the file's SHA-256 on Flowsta's own signing DHT,
not on this DNA.

Browser notes: Safari, Brave and phones can't reach the Vault on localhost. The SDK offers relay
login for those cases.

## Not done yet

- A UI. Qwik (as in ProofPoll) or Svelte, packaged with Kangaroo-Electron.
- Cancellations, host-to-guest reviews, and messaging, which could use ProofPoll's encrypted-entry
  pattern.
- Sharding very busy coarse index cells.
- Flagging reviews from agents linked to the host's own identity.
- A membrane, if the network shouldn't be open to everyone.
