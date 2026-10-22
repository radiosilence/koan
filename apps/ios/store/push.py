# /// script
# requires-python = ">=3.12"
# dependencies = ["pyjwt[crypto]"]
# ///
"""Push the App Store listing in listing.toml through the App Store Connect API.

    push.py [--platform ios|tvos] [--version X.Y.Z] [--attachment FILE] [--submit] [iphone=DIR ipad=DIR tv=DIR]

Updates the app information, the platform's version in preparation (created if
there is none), its text, the App Review details, and, when screenshot
directories are given, replaces the screenshots. Attaches the newest processed
build of the version, the workspace's unless given.

`--attachment` replaces the App Review attachments with FILE, such as the
recording App Review asks for. An attachment stays with the version record,
which is reused until a version is released, so it is given once.

`--submit` sends the version to App Review. A submission already waiting is
cancelled first, so the version can be edited and submitted again; one that
App Review answered is resubmitted with its items marked resolved.

Needs APPLE_API_KEY_PATH, APPLE_API_KEY_ID and APPLE_API_ISSUER_ID, as
`just ios-testflight` does, and APPLE_REVIEW_PHONE until App Review has one.
"""

import argparse
import hashlib
import json
import os
import sys
import time
import tomllib
import urllib.error
import urllib.request
from pathlib import Path

import jwt

API = "https://api.appstoreconnect.apple.com"
BUNDLE_ID = "cc.blit.koan"
# The display types App Store Connect requires: 6.9-inch iPhone, 13-inch iPad
# and Apple TV.
DISPLAY_TYPES = {"iphone": "APP_IPHONE_67", "ipad": "APP_IPAD_PRO_3GEN_129", "tv": "APP_APPLE_TV"}
PLATFORMS = {"ios": "IOS", "tvos": "TV_OS"}
# Submissions that can be cancelled to edit the version again.
WAITING = {"WAITING_FOR_REVIEW", "READY_FOR_REVIEW"}
EDITABLE = {"PREPARE_FOR_SUBMISSION", "DEVELOPER_REJECTED", "REJECTED", "METADATA_REJECTED"}


def token():
    now = int(time.time())
    return jwt.encode(
        {"iss": os.environ["APPLE_API_ISSUER_ID"], "iat": now, "exp": now + 1200, "aud": "appstoreconnect-v1"},
        Path(os.environ["APPLE_API_KEY_PATH"]).read_text(),
        algorithm="ES256",
        headers={"kid": os.environ["APPLE_API_KEY_ID"], "typ": "JWT"},
    )


def call(method, path, body=None):
    # App Store Connect answers 500 now and then and succeeds on a retry.
    for attempt in range(4):
        req = urllib.request.Request(
            API + path,
            method=method,
            data=json.dumps(body).encode() if body is not None else None,
            headers={"Authorization": f"Bearer {token()}", "Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req) as r:
                text = r.read()
                return json.loads(text) if text else {}
        except urllib.error.HTTPError as e:
            if e.code >= 500 and attempt < 3:
                time.sleep(2 * (attempt + 1))
                continue
            sys.exit(f"{method} {path}: {e.code}\n{e.read().decode()}")


def patch(kind, id, attributes, relationships=None):
    data = {"type": kind, "id": id, "attributes": attributes}
    if relationships:
        data["relationships"] = relationships
    return call("PATCH", f"/v1/{kind}/{id}", {"data": data})


def create(kind, attributes, relationships):
    data = {"type": kind, "attributes": attributes, "relationships": relationships}
    return call("POST", f"/v1/{kind}", {"data": data})["data"]


def rel(kind, id):
    return {"data": {"type": kind, "id": id}}


def localization(items, locale):
    return next((i for i in items if i["attributes"]["locale"] == locale), None)


def workspace_version():
    cargo = tomllib.loads(Path(__file__).parents[3].joinpath("Cargo.toml").read_text())
    return cargo["workspace"]["package"]["version"]


def upload(kind, attributes, relationships, data):
    """Create an asset, send its bytes through the upload operations, commit it."""
    asset = create(kind, attributes, relationships)
    for op in asset["attributes"]["uploadOperations"]:
        part = data[op["offset"] : op["offset"] + op["length"]]
        req = urllib.request.Request(op["url"], method=op["method"], data=part)
        for header in op["requestHeaders"]:
            req.add_header(header["name"], header["value"])
        urllib.request.urlopen(req).read()
    patch(kind, asset["id"], {"uploaded": True, "sourceFileChecksum": hashlib.md5(data).hexdigest()})
    return asset


def upload_screenshots(localization_id, kind, directory):
    display = DISPLAY_TYPES[kind]
    files = sorted(f for f in Path(directory).iterdir() if f.suffix.lower() in {".png", ".jpg", ".jpeg"})
    if not files:
        sys.exit(f"no screenshots in {directory}")
    sets = call("GET", f"/v1/appStoreVersionLocalizations/{localization_id}/appScreenshotSets")["data"]
    existing = next((s for s in sets if s["attributes"]["screenshotDisplayType"] == display), None)
    if existing:
        for shot in call("GET", f"/v1/appScreenshotSets/{existing['id']}/appScreenshots")["data"]:
            call("DELETE", f"/v1/appScreenshots/{shot['id']}")
        set_id = existing["id"]
    else:
        set_id = create(
            "appScreenshotSets",
            {"screenshotDisplayType": display},
            {"appStoreVersionLocalization": rel("appStoreVersionLocalizations", localization_id)},
        )["id"]
    for file in files:
        data = file.read_bytes()
        upload(
            "appScreenshots",
            {"fileName": file.name, "fileSize": len(data)},
            {"appScreenshotSet": rel("appScreenshotSets", set_id)},
            data,
        )
        print(f"  {kind}: {file.name}")


def attach(detail_id, file):
    for old in call("GET", f"/v1/appStoreReviewDetails/{detail_id}/appStoreReviewAttachments")["data"]:
        call("DELETE", f"/v1/appStoreReviewAttachments/{old['id']}")
    data = Path(file).read_bytes()
    upload(
        "appStoreReviewAttachments",
        {"fileName": Path(file).name, "fileSize": len(data)},
        {"appStoreReviewDetail": rel("appStoreReviewDetails", detail_id)},
        data,
    )
    print(f"review attachment {Path(file).name} uploaded")


def settle(submission_id):
    """Wait for a cancelled submission to finish cancelling."""
    for _ in range(60):
        state = call("GET", f"/v1/reviewSubmissions/{submission_id}")["data"]["attributes"]["state"]
        if state == "COMPLETE":
            return
        time.sleep(5)
    sys.exit(f"submission {submission_id} is still {state}")


def withdraw(app_id, platform):
    """Cancel a submission waiting for review, which locks the version, and
    return one App Review answered, to be resubmitted."""
    submissions = call("GET", f"/v1/reviewSubmissions?filter[app]={app_id}&filter[platform]={platform}&limit=20")["data"]
    answered = None
    for sub in submissions:
        state = sub["attributes"]["state"]
        if state in WAITING:
            patch("reviewSubmissions", sub["id"], {"canceled": True})
            settle(sub["id"])
            print(f"submission {sub['id']} cancelled")
        elif state == "UNRESOLVED_ISSUES":
            answered = sub
    return answered


def submit(app_id, platform, version_id, answered):
    if answered:
        items = call("GET", f"/v1/reviewSubmissions/{answered['id']}/items")["data"]
        for item in items:
            patch("reviewSubmissionItems", item["id"], {"resolved": True})
        submission = answered
    else:
        submission = create("reviewSubmissions", {"platform": platform}, {"app": rel("apps", app_id)})
        # Screenshots still processing refuse the item for a minute or two.
        for attempt in range(12):
            try:
                create(
                    "reviewSubmissionItems",
                    {},
                    {
                        "reviewSubmission": rel("reviewSubmissions", submission["id"]),
                        "appStoreVersion": rel("appStoreVersions", version_id),
                    },
                )
                break
            except SystemExit as e:
                if "ASSET_IN_POST_PROCESSING" not in str(e) or attempt == 11:
                    raise
                time.sleep(20)
    patch("reviewSubmissions", submission["id"], {"submitted": True})
    print(f"submitted for review: {submission['id']}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", choices=PLATFORMS, default="ios")
    parser.add_argument("--version", default=workspace_version())
    parser.add_argument("--attachment")
    parser.add_argument("--submit", action="store_true")
    parser.add_argument("screenshots", nargs="*", help="KIND=DIR, KIND one of " + ", ".join(DISPLAY_TYPES))
    args = parser.parse_args()
    platform = PLATFORMS[args.platform]
    screenshots = dict(arg.split("=", 1) for arg in args.screenshots)

    listing = tomllib.loads(Path(__file__).with_name("listing.toml").read_text())
    if args.platform == "tvos":
        for section, fields in listing["tv"].items():
            listing[section] = {**listing[section], **fields}
    locale = listing["locale"]

    app = call("GET", f"/v1/apps?filter[bundleId]={BUNDLE_ID}")["data"][0]
    app_id = app["id"]

    info = next(
        i for i in call("GET", f"/v1/apps/{app_id}/appInfos")["data"] if i["attributes"]["state"] != "READY_FOR_DISTRIBUTION"
    )
    patch("appInfos", info["id"], {}, {"primaryCategory": rel("appCategories", listing["app"]["primary_category"])})
    info_loc = localization(call("GET", f"/v1/appInfos/{info['id']}/appInfoLocalizations")["data"], locale)
    patch(
        "appInfoLocalizations",
        info_loc["id"],
        {
            "name": listing["app"]["name"],
            "subtitle": listing["app"]["subtitle"],
            "privacyPolicyUrl": listing["app"]["privacy_policy_url"],
        },
    )
    print("app information updated")

    version_string = args.version
    builds = call(
        "GET",
        f"/v1/builds?filter[app]={app_id}&filter[preReleaseVersion.version]={version_string}"
        f"&filter[preReleaseVersion.platform]={platform}&filter[processingState]=VALID&sort=-uploadedDate&limit=1",
    )["data"]
    if args.submit and not builds:
        sys.exit(f"no processed {args.platform} build of {version_string} to submit")
    answered = withdraw(app_id, platform) if args.submit else None

    versions = call("GET", f"/v1/apps/{app_id}/appStoreVersions?filter[platform]={platform}")["data"]
    version = next((v for v in versions if v["attributes"]["appStoreState"] in EDITABLE), None)
    if version is None:
        version = create(
            "appStoreVersions", {"platform": platform, "versionString": version_string}, {"app": rel("apps", app_id)}
        )
    elif version["attributes"]["versionString"] != version_string:
        patch("appStoreVersions", version["id"], {"versionString": version_string})
    patch(
        "appStoreVersions",
        version["id"],
        {"copyright": listing["version"]["copyright"], "releaseType": "AFTER_APPROVAL"},
    )

    v = listing["version"]
    loc = localization(call("GET", f"/v1/appStoreVersions/{version['id']}/appStoreVersionLocalizations")["data"], locale)
    text = {
        "description": v["description"],
        "keywords": v["keywords"],
        "promotionalText": v["promotional_text"],
        "supportUrl": v["support_url"],
        "marketingUrl": v["marketing_url"],
    }
    if loc is None:
        loc = create(
            "appStoreVersionLocalizations",
            {"locale": locale, **text},
            {"appStoreVersion": rel("appStoreVersions", version["id"])},
        )
    else:
        patch("appStoreVersionLocalizations", loc["id"], text)
    print(f"{args.platform} {version_string} text updated")

    r = listing["review"]
    review = {
        "contactFirstName": r["first_name"],
        "contactLastName": r["last_name"],
        "contactEmail": r["email"],
        "demoAccountRequired": r["demo_required"],
        "demoAccountName": r["demo_user"],
        "demoAccountPassword": r["demo_password"],
        "notes": r["notes"],
    }
    if phone := os.environ.get("APPLE_REVIEW_PHONE"):
        review["contactPhone"] = phone
    detail = call("GET", f"/v1/appStoreVersions/{version['id']}/appStoreReviewDetail").get("data")
    if detail:
        patch("appStoreReviewDetails", detail["id"], review)
    else:
        detail = create("appStoreReviewDetails", review, {"appStoreVersion": rel("appStoreVersions", version["id"])})
    print("review details updated")
    if args.attachment:
        attach(detail["id"], args.attachment)

    for kind, directory in screenshots.items():
        upload_screenshots(loc["id"], kind, directory)

    if builds:
        patch("appStoreVersions", version["id"], {}, {"build": rel("builds", builds[0]["id"])})
        print(f"build {builds[0]['attributes']['version']} attached")
    else:
        print(f"no processed build of {version_string} yet")

    if args.submit:
        submit(app_id, platform, version["id"], answered)


if __name__ == "__main__":
    main()
