# Link shortener — what it has to do

A small HTTP service. Build it from nothing in this directory. Any language,
any stack, as long as it runs on this machine and the checks below pass.

## Endpoints

### `GET /healthz`
Returns `200` once the service is ready to take traffic. Body does not matter.

### `POST /api/links`
Body is JSON: `{"url": "https://example.com/some/page"}`.

- On success returns `201` and JSON `{"code": "...", "short_url": "..."}`.
  `code` is 6 to 10 characters, letters and digits only.
- Posting a URL that already has a code returns `200` and **the same code**.
  The same link never gets two codes.
- A URL that is not `http://` or `https://`, a missing `url` field, an empty
  string, or a body that is not valid JSON returns `400` and JSON with an
  `error` field. It must not return `500`.

### `GET /{code}`
Redirects to the original URL.

- `301`, `302`, `307` or `308`, with a `Location` header holding the original
  URL **exactly** as it was submitted — query string, encoding and fragment
  unchanged.
- Each redirect counts as one hit.
- An unknown code returns `404`.

### `GET /api/links/{code}`
Returns `200` and JSON `{"url": "...", "hits": n}`.

- `hits` is the number of redirects served for that code. Looking a link up
  here is **not** a hit.
- An unknown code returns `404`.

## Durability

Links and hit counts survive the service being stopped and started again.
Nothing is kept only in memory.

## How it gets started

Write `serve.json` in this directory so the service can be launched without
guessing:

```json
{
  "command": ["python3", "app.py"],
  "port": 8080,
  "health_path": "/healthz"
}
```

`command` is an argv list run with this directory as the working directory.
`port` is the port the service listens on. If a build step is needed first,
put it in `build.json` in the same shape (`{"command": [...]}`); it runs to
completion before `command` starts, and it must exit 0.
