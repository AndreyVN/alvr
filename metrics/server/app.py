"""FastAPI server for ALVR streaming + hardware metrics.

Two exporters on the streamer side push to this service:

* `metrics_export.url`     → POST /metrics      → alvr.streaming_metrics
* `metrics_export.hw_url`  → POST /hw_metrics   → alvr.hw_* tables

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

from .db import get_client, insert, insert_hw
from .models import HwSnapshot, Snapshot

log = logging.getLogger("alvr.metrics.ingest")

app = FastAPI(title="ALVR metrics ingest", version="1.1.0")


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


@app.post("/hw_metrics", status_code=status.HTTP_204_NO_CONTENT)
def ingest_hw_metrics(snapshot: HwSnapshot) -> Response:
    try:
        insert_hw(snapshot)
    except Exception as e:
        log.exception("clickhouse hw insert failed")
        raise HTTPException(status_code=500, detail=f"insert failed: {e}") from e
    return Response(status_code=status.HTTP_204_NO_CONTENT)
