"""Fail-closed GitHub Release rate guard for release automation."""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import Callable, Iterable
from urllib.parse import urlencode, urlsplit
from urllib.request import Request, urlopen

HOURLY_LIMIT = 3
DAILY_LIMIT = 12
PAGE_SIZE = 100
TRUSTED_API_URL = "https://api.github.com"
REPOSITORY_PATTERN = r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$"


class ReleaseRateError(RuntimeError):
    """Raised when release history cannot be trusted."""


@dataclass(frozen=True)
class RateDecision:
    """Counts and decision returned by the guard."""

    hourly_count: int
    daily_count: int
    hourly_limit: int = HOURLY_LIMIT
    daily_limit: int = DAILY_LIMIT

    @property
    def allowed(self) -> bool:
        return self.hourly_count < self.hourly_limit and self.daily_count < self.daily_limit

    def message(self) -> str:
        return (
            f"GitHub Release rate window: {self.hourly_count}/{self.hourly_limit} in the "
            f"last hour, {self.daily_count}/{self.daily_limit} in the last 24 hours"
        )


def parse_timestamp(value: object) -> datetime:
    """Parse a GitHub ISO-8601 timestamp and reject ambiguous input."""

    if not isinstance(value, str) or not value:
        raise ReleaseRateError("release timestamp is missing or not a string")
    try:
        timestamp = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise ReleaseRateError(f"invalid release timestamp: {value!r}") from error
    if timestamp.tzinfo is None:
        raise ReleaseRateError(f"release timestamp has no timezone: {value!r}")
    return timestamp.astimezone(timezone.utc)


def release_timestamp(release: object) -> datetime:
    """Return the creation/publication timestamp used for rate counting."""

    if not isinstance(release, dict):
        raise ReleaseRateError("GitHub Releases response contains a non-object entry")
    published_at = release.get("published_at")
    created_at = release.get("created_at")
    if published_at:
        return parse_timestamp(published_at)
    if created_at:
        return parse_timestamp(created_at)
    raise ReleaseRateError("GitHub Release entry has neither published_at nor created_at")


def count_releases(
    releases: Iterable[object],
    *,
    now: datetime,
    hourly_limit: int = HOURLY_LIMIT,
    daily_limit: int = DAILY_LIMIT,
) -> RateDecision:
    """Count all release types in inclusive rolling windows."""

    now = now.astimezone(timezone.utc)
    hour_start = now - timedelta(hours=1)
    day_start = now - timedelta(days=1)
    hourly_count = 0
    daily_count = 0
    for release in releases:
        timestamp = release_timestamp(release)
        if timestamp >= day_start:
            daily_count += 1
        if timestamp >= hour_start:
            hourly_count += 1
    return RateDecision(
        hourly_count=hourly_count,
        daily_count=daily_count,
        hourly_limit=hourly_limit,
        daily_limit=daily_limit,
    )


def _read_json_response(response: object) -> object:
    try:
        status = getattr(response, "status", 200)
        payload = response.read()
    except (AttributeError, OSError) as error:  # pragma: no cover - exercised through fetch_releases
        raise ReleaseRateError(f"GitHub Releases API response could not be read: {error}") from error
    if status != 200:
        raise ReleaseRateError(f"GitHub Releases API returned HTTP {status}")
    try:
        return json.loads(payload)
    except (TypeError, json.JSONDecodeError) as error:
        raise ReleaseRateError("GitHub Releases API returned malformed JSON") from error


def _validate_repository(repository: str) -> str:
    if not re.fullmatch(REPOSITORY_PATTERN, repository):
        raise ReleaseRateError("repository must have the form owner/name")
    return repository


def _validate_api_url(api_url: str) -> str:
    parts = urlsplit(api_url)
    if (
        parts.scheme != "https"
        or parts.netloc != "api.github.com"
        or parts.path.rstrip("/")
        or parts.query
        or parts.fragment
    ):
        raise ReleaseRateError("GitHub Releases API URL must be https://api.github.com")
    return TRUSTED_API_URL


def _release_page(payload: object) -> list[object]:
    if not isinstance(payload, list):
        raise ReleaseRateError("GitHub Releases API returned a non-array response")
    return payload


def fetch_releases(
    repository: str,
    token: str,
    *,
    api_url: str = TRUSTED_API_URL,
    opener: Callable[..., object] = urlopen,
) -> list[object]:
    """Fetch every release page, rejecting incomplete or malformed responses."""

    if not repository or not token:
        raise ReleaseRateError("repository and GitHub token are required")
    repository = _validate_repository(repository)
    api_url = _validate_api_url(api_url)
    releases: list[object] = []
    page = 1
    while True:
        query = urlencode({"per_page": PAGE_SIZE, "page": page})
        request = Request(
            f"{api_url.rstrip('/')}/repos/{repository}/releases?{query}",
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": "Bearer " + token,
                "User-Agent": "bhtune-release-rate-limit",
                "X-GitHub-Api-Version": "2022-11-28",
            },
        )
        try:
            response = opener(request, timeout=20)
            payload = _read_json_response(response)
        except OSError as error:
            raise ReleaseRateError(f"GitHub Releases API request failed: {error}") from error
        payload = _release_page(payload)
        for release in payload:
            release_timestamp(release)
        releases.extend(payload)
        if len(payload) < PAGE_SIZE:
            return releases
        page += 1
        if page > 1000:
            raise ReleaseRateError("GitHub Releases API pagination exceeded the safety limit")


def check_repository(
    repository: str,
    token: str,
    *,
    now: datetime | None = None,
    api_url: str = TRUSTED_API_URL,
    opener: Callable[..., object] = urlopen,
) -> RateDecision:
    """Fetch release history and return a fail-closed rate decision."""

    releases = fetch_releases(repository, token, api_url=api_url, opener=opener)
    return count_releases(releases, now=now or datetime.now(timezone.utc))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY"))
    parser.add_argument("--token", default=os.environ.get("GITHUB_TOKEN"))
    parser.add_argument("--api-url", default=os.environ.get("GITHUB_API_URL", TRUSTED_API_URL))
    parser.add_argument("--now", help="UTC ISO-8601 timestamp for deterministic checks")
    args = parser.parse_args()

    try:
        now = datetime.fromisoformat(args.now.replace("Z", "+00:00")) if args.now else None
        decision = check_repository(
            args.repository or "",
            args.token or "",
            now=now,
            api_url=args.api_url,
        )
    except (ReleaseRateError, ValueError) as error:
        print(f"release-rate-limit: {error}", file=sys.stderr)
        return 1

    print(decision.message())
    if not decision.allowed:
        print("release-rate-limit: refusing release operation", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
