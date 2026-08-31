# Playground

A single page that exercises every endpoint the service exposes: browse a
title's artwork as thumbnails, pick a poster and logo, choose a preset and
badges, render, and read back the caching headers.

Plain HTML, CSS and JavaScript — no build step. That is deliberate: a
playground for a service whose selling point is that it is easy to call should
not need a toolchain to look at.

## Running it locally

Two servers: the API, and anything that serves static files.

```sh
# the service
POSTER_TMDB_API_KEY=… cargo run --release

# the page, from this directory
python3 -m http.server 4173
```

Then open <http://localhost:4173>. The **Base URL** field at the top points at
`http://localhost:8080` by default; change it to aim at a deployed instance.

## Why the service needs CORS

The page runs on a different origin from the API — `localhost:4173` against
`localhost:8080`, or a GitHub Pages domain against wherever the service lives.
A browser refuses cross-origin responses unless the service says otherwise, so
the API sends `Access-Control-Allow-Origin`.

It defaults to any origin, which is safe here in a way it would not be for most
services: CORS protects *ambient authority* — cookies, sessions, an
`Authorization` header the browser attaches on the caller's behalf — and this
API has none of those. `POSTER_CORS_ALLOW_ORIGINS` narrows it for a deployment
that is not meant to be public.

A service older than the CORS change will fail every request from this page
before reaching any endpoint. The page says so rather than showing a bare
network error, because a browser cannot tell a page *why* a cross-origin
request failed.

## Deployment

`.github/workflows/pages.yml` publishes this directory to GitHub Pages on any
push to `main` that touches it. Pages must be enabled once, in
**Settings → Pages → Source: GitHub Actions**.

A deployed page still needs a reachable API. Point **Base URL** at one, and set
`POSTER_CORS_ALLOW_ORIGINS` on that service to the Pages origin if it is not
meant to serve everyone.
