# Postman collection

Two files, both importable directly:

| File | What it is |
|---|---|
| `poster-service.postman_collection.json` | 21 requests across 5 folders |
| `poster-service.postman_environment.json` | `base_url` and the sample TMDB identifiers |

## Import

In Postman: **Import** → drop both files in → select **Poster Service — local**
from the environment picker.

## Run it

Start the service with a TMDB credential, since resolving an identifier to
artwork is an authenticated call:

```sh
POSTER_TMDB_API_KEY=… cargo run
```

Then run **1. Discover › Browse artwork (film)**, followed by any request under
**2. Create poster**. Requests chain through collection variables, so nothing
needs copying by hand:

```
Browse artwork  ──sets──▶  poster_path, logo_path
Create poster   ──sets──▶  poster_key
Get poster      ──uses──▶  poster_key
```

`Choose the logo yourself` uses `logo_path`, so browse first or it has nothing
to send.

## No credentials live here

The TMDB credential belongs to the *service*, not to its callers, so nothing in
these files carries one. That is why they are safe to commit, and why a poster
request against a service started without one returns
`tmdb_credential_missing` rather than working.

## Running it from the command line

The collection is also a test suite. `newman` runs it end to end and asserts
status codes, the `problem+json` shape of every error, and that a second fetch
of the same poster reports `x-cache: HIT`:

```sh
npm install -g newman
newman run postman/poster-service.postman_collection.json \
       -e postman/poster-service.postman_environment.json
```

Against a service with a valid credential that is 21 requests and 45
assertions.

## Variables

| Variable | Default | |
|---|---|---|
| `base_url` | `http://localhost:8080` | Change for a deployed instance |
| `movie_id` | `27205` | Inception |
| `tv_id` | `1396` | Breaking Bad |
| `language` | `en` | Orders artwork by language preference |
