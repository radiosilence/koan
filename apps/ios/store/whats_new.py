# /// script
# requires-python = ">=3.12"
# dependencies = ["pyjwt[crypto]"]
# ///
"""Set a TestFlight build's "What to Test" from the changelog.

`whats_new.py BUILD [PLATFORM]` waits for build BUILD of the workspace version,
on PLATFORM (`IOS`, the default, or `TV_OS`), to finish
processing, then writes that version's CHANGELOG.md section, as plain text, to
the build's en-GB localization: what testers see when they install it.

Needs the same APPLE_API_* environment as push.py.
"""

import re
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from push import call, create, localization, rel, patch, workspace_version  # noqa: E402

BUNDLE_ID = "cc.blit.koan"
LOCALE = "en-GB"
# App Store Connect caps the field at 4000 characters.
LIMIT = 4000
# Processing usually takes a few minutes; past this the build has a problem
# the upload step already reported.
PATIENCE = 45 * 60


def notes(version):
    # Generated rather than read: the committed file has no Unreleased block,
    # which the fallback below looks for.
    script = Path(__file__).parents[3] / "scripts" / "changelog.py"
    text = subprocess.run([sys.executable, script, "--stdout"], check=True, capture_output=True, text=True).stdout
    match = re.search(rf"^## {re.escape(version)}\n(.*?)(?=^## |\Z)", text, re.S | re.M)
    body = match.group(1) if match else ""
    if not body.strip():
        # Released before the section was written: the unreleased one is it.
        match = re.search(r"^## Unreleased\n(.*?)(?=^## |\Z)", text, re.S | re.M)
        body = match.group(1) if match else ""
    body = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", body)  # links to their text
    body = re.sub(r"\*\*|`", "", body)
    body = re.sub(r"^### (.+)$", lambda m: m.group(1).upper(), body, flags=re.M)
    body = re.sub(r"\n{3,}", "\n\n", body).strip()
    if len(body) > LIMIT:
        cut = body.rfind("\n", 0, LIMIT - 40)
        body = body[:cut] + "\n\n…and more in CHANGELOG.md"
    return body


def main():
    build_number = sys.argv[1]
    platform = sys.argv[2] if len(sys.argv) > 2 else "IOS"
    version = workspace_version()
    text = notes(version)
    if not text:
        sys.exit(f"no changelog section for {version}")

    app = call("GET", f"/v1/apps?filter[bundleId]={BUNDLE_ID}")["data"][0]
    deadline = time.time() + PATIENCE
    while True:
        builds = call(
            "GET",
            f"/v1/builds?filter[app]={app['id']}&filter[version]={build_number}"
            f"&filter[preReleaseVersion.version]={version}"
            f"&filter[preReleaseVersion.platform]={platform}",
        )["data"]
        state = builds[0]["attributes"]["processingState"] if builds else "NOT_YET_LISTED"
        if state == "VALID":
            break
        if state in ("FAILED", "INVALID"):
            sys.exit(f"build {build_number} is {state}")
        if time.time() > deadline:
            sys.exit(f"build {build_number} still {state} after {PATIENCE // 60} minutes")
        print(f"build {build_number}: {state}, waiting")
        time.sleep(30)

    build = builds[0]
    existing = call("GET", f"/v1/builds/{build['id']}/betaBuildLocalizations")["data"]
    held = localization(existing, LOCALE)
    if held:
        patch("betaBuildLocalizations", held["id"], {"whatsNew": text})
    else:
        create("betaBuildLocalizations", {"locale": LOCALE, "whatsNew": text}, {"build": rel("builds", build["id"])})
    print(f"What to Test set for {version} ({build_number}): {len(text)} characters")


if __name__ == "__main__":
    main()
