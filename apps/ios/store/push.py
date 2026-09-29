# /// script
# requires-python = ">=3.12"
# dependencies = ["pyjwt[crypto]"]
# ///
"""Push the App Store listing in listing.toml through the App Store Connect API.

Updates the app information, the version in preparation (created from the
workspace version if there is none), its text, the App Review details, and,
when screenshot directories are given, replaces the screenshots. Attaches the
newest processed build of that version. Never submits for review.

Needs APPLE_API_KEY_PATH, APPLE_API_KEY_ID and APPLE_API_ISSUER_ID, as
`just ios-testflight` does, and APPLE_REVIEW_PHONE until App Review has one.
"""

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
# The display types App Store Connect requires for an iPhone and iPad app:
# 6.9-inch iPhone and 13-inch iPad.
DISPLAY_TYPES = {"iphone": "APP_IPHONE_67", "ipad": "APP_IPAD_PRO_3GEN_129"}
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


def upload_screenshots(localization_id, kind, directory):
    display = DISPLAY_TYPES[kind]
    files = sorted(Path(directory).glob("*.png"))
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
        shot = create(
            "appScreenshots",
            {"fileName": file.name, "fileSize": len(data)},
            {"appScreenshotSet": rel("appScreenshotSets", set_id)},
        )
        for op in shot["attributes"]["uploadOperations"]:
            part = data[op["offset"] : op["offset"] + op["length"]]
            req = urllib.request.Request(op["url"], method=op["method"], data=part)
            for header in op["requestHeaders"]:
                req.add_header(header["name"], header["value"])
            urllib.request.urlopen(req).read()
        patch("appScreenshots", shot["id"], {"uploaded": True, "sourceFileChecksum": hashlib.md5(data).hexdigest()})
        print(f"  {kind}: {file.name}")


def main():
    listing = tomllib.loads(Path(__file__).with_name("listing.toml").read_text())
    locale = listing["locale"]
    screenshots = dict(arg.split("=", 1) for arg in sys.argv[1:])

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

    version_string = workspace_version()
    versions = call("GET", f"/v1/apps/{app_id}/appStoreVersions?filter[platform]=IOS")["data"]
    version = next((v for v in versions if v["attributes"]["appStoreState"] in EDITABLE), None)
    if version is None:
        version = create(
            "appStoreVersions", {"platform": "IOS", "versionString": version_string}, {"app": rel("apps", app_id)}
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
    print(f"version {version_string} text updated")

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
        create("appStoreReviewDetails", review, {"appStoreVersion": rel("appStoreVersions", version["id"])})
    print("review details updated")

    for kind, directory in screenshots.items():
        upload_screenshots(loc["id"], kind, directory)

    builds = call(
        "GET",
        f"/v1/builds?filter[app]={app_id}&filter[preReleaseVersion.version]={version_string}"
        "&filter[processingState]=VALID&sort=-uploadedDate&limit=1",
    )["data"]
    if builds:
        patch("appStoreVersions", version["id"], {}, {"build": rel("builds", builds[0]["id"])})
        print(f"build {builds[0]['attributes']['version']} attached")
    else:
        print(f"no processed build of {version_string} yet")


if __name__ == "__main__":
    main()
