"""FastAPI server for ALVR streaming metrics.

The ALVR server's metrics exporter POSTs aggregated snapshots to
`metrics_export.url`. Point that URL at this service (POST /metrics) and
each snapshot is flattened and inserted into `alvr.streaming_metrics`.

Run:
    pip install -r metrics/server/requirements.txt
    uvicorn metrics.server.app:app --host 0.0.0.0 --port 8086

Connection params are read from CLICKHOUSE_HOST / _PORT / _USER /
_PASSWORD / _DATABASE env vars (defaults: 127.0.0.1:8123, default user,
empty password, alvr database).
"""

from __future__ import annotations

import logging

from fastapi import FastAPI, HTTPException, Response, status

from .db import get_client, insert
from .models import Snapshot

log = logging.getLogger("alvr.metrics.ingest")

app = FastAPI(title="ALVR metrics ingest", version="1.0.0")


@app.get("/health")
def health() -> dict:
    try:
        get_client().command("SELECT 1")
    except Exception as e:  # pragma: no cover — surfaced verbatim for the operator
        raise HTTPException(status_code=503, detail=f"clickhouse unreachable: {e}") from e
    return {"status": "ok"}


@app.post("/metrics", status_code=status.HTTP_204_NO_CONTENT)
def ingest_metrics(snapshot: Snapshot) -> Response:
    try:
        insert(snapshot)
    except Exception as e:
        log.exception("clickhouse insert failed")
        raise HTTPException(status_code=500, detail=f"insert failed: {e}") from e
    return Response(status_code=status.HTTP_204_NO_CONTENT)
