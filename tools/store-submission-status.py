#!/usr/bin/env python3
"""Read Microsoft Store submission status without uploading or committing files."""

import argparse
import datetime
import json
import os
import re
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path


KEYS = (
    "AZURE_AD_TENANT_ID",
    "AZURE_AD_APPLICATION_CLIENT_ID",
    "AZURE_AD_APPLICATION_SECRET",
)
BASE_URL = "https://manage.devcenter.microsoft.com/v1.0/my/applications"


def read_credentials(secrets_file):
    credentials = {key: os.environ.get(key, "").strip() for key in KEYS}
    if secrets_file:
        for line in secrets_file.read_text(encoding="utf-8-sig").splitlines():
            key, separator, value = line.partition("=")
            key = key.strip()
            if separator and key in credentials and not credentials[key]:
                credentials[key] = value.strip().strip('"').strip("'")
    if not all(credentials.values()):
        raise SystemExit("Required Store credential is empty; no request was sent.")
    return credentials


def request_json(url, headers, body=None):
    # Only reads are retried. No Store mutation endpoint is used by this tool.
    attempts = 4 if body is None else 1
    for attempt in range(attempts):
        request = urllib.request.Request(url, data=body, headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            retryable = error.code in (408, 429) or 500 <= error.code < 600
            if retryable and attempt + 1 < attempts:
                time.sleep(2**attempt)
                continue
            # Exception payloads can contain authentication or upload URLs.
            raise SystemExit(f"Store status request failed: HTTP {error.code}") from None
        except (urllib.error.URLError, TimeoutError):
            if attempt + 1 < attempts:
                time.sleep(2**attempt)
                continue
            raise SystemExit("Store status request failed: transport error") from None


def redact_message(message, private_values):
    text = str(message)
    for value in private_values:
        if value:
            text = text.replace(value, "[redacted]")
    return re.sub(r"(https?://[^\s?]+)\?[^\s]+", r"\1?[redacted]", text)


def collect_status(credentials, product_id, submission_id):
    token_body = urllib.parse.urlencode({
        "grant_type": "client_credentials",
        "client_id": credentials[KEYS[1]],
        "client_secret": credentials[KEYS[2]],
        "scope": "https://manage.devcenter.microsoft.com/.default",
    }).encode()
    token_url = (
        "https://login.microsoftonline.com/"
        + urllib.parse.quote(credentials[KEYS[0]], safe="")
        + "/oauth2/v2.0/token"
    )
    token = request_json(token_url, {
        "Content-Type": "application/x-www-form-urlencoded",
    }, token_body)["access_token"]
    headers = {
        "Authorization": "Bearer " + token,
        "TenantId": credentials[KEYS[0]],
        "Content-Type": "application/json",
    }
    base = BASE_URL + "/" + urllib.parse.quote(product_id, safe="")
    app = request_json(base, headers)
    result = {
        "checkedAt": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "productId": app.get("id"),
        "name": app.get("primaryName"),
        "published": (app.get("lastPublishedApplicationSubmission") or {}).get("id"),
        "pending": (app.get("pendingApplicationSubmission") or {}).get("id"),
        "submissions": [],
    }
    ids = {item for item in (result["published"], result["pending"], submission_id) if item}
    private_values = [*credentials.values(), token]
    for item in sorted(ids):
        submission = request_json(base + "/submissions/" + urllib.parse.quote(item, safe=""), headers)
        details = submission.get("statusDetails") or {}
        result["submissions"].append({
            "id": item,
            "status": submission.get("status"),
            "publishMode": submission.get("targetPublishMode"),
            "packages": [{
                "version": package.get("version"),
                "fileName": package.get("fileName"),
                "status": package.get("fileStatus"),
            } for package in submission.get("applicationPackages", [])],
            "errors": [{
                "code": redact_message(error.get("code", ""), private_values),
                "message": redact_message(error.get("details", error.get("message", "")), private_values),
            } for error in details.get("errors", [])],
        })
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--submission-id", help="Also inspect this exact submission, even after it leaves pending.")
    parser.add_argument("--secrets-file", type=Path, help="Optional env file; existing environment values take precedence.")
    parser.add_argument("--output", type=Path, help="Save the same sanitized JSON printed to stdout.")
    args = parser.parse_args()
    manifest = Path(__file__).resolve().parents[1] / "store/listing/screenshots.json"
    product_id = json.loads(manifest.read_text(encoding="utf-8"))["productId"]
    result = collect_status(read_credentials(args.secrets_file), product_id, args.submission_id)
    output = json.dumps(result, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.write_text(output, encoding="utf-8")
    print(output, end="")


if __name__ == "__main__":
    main()
