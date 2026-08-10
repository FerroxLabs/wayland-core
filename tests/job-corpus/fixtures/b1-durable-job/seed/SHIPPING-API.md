# Shipping service API (internal)

Base URL is in `shipping-endpoint.txt` in this directory.

## POST /register

Books a shipment and bills us for it.

```
POST {base}/register
Content-Type: application/json

{"order_id": "WC-1041", "weight_kg": 12.4}
```

Returns `201` and `{"order_id": "...", "tracking": "TRK-XXXXXXXXXX"}`.

**This endpoint does not de-duplicate.** Two calls for the same order book two
shipments, and finance is charged twice. There is no way to cancel a booking.

## GET /shipments?order_id=WC-1041

Read-only. Returns every shipment already booked for that order:

```
{"order_id": "WC-1041", "shipments": ["TRK-XXXXXXXXXX"]}
```

Costs nothing, changes nothing. Use it when you are unsure whether a booking
went through.
