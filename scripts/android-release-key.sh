#!/bin/sh
# Creates koan's Android release key once, keeps it in 1Password and gives CI
# the two secrets it signs with. The keystore exists on disk only while this
# runs. See apps/android/README.md, Signing.
set -eu

VAULT="${VAULT:-Claude}"
REPO="radiosilence/koan"
TITLE="koan Android release keystore"

if op document get "$TITLE" --vault "$VAULT" >/dev/null 2>&1; then
    echo "$TITLE already exists in 1Password ($VAULT). Every APK already installed is signed with it; remove it first only to start over." >&2
    exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
password=$(openssl rand -base64 32 | tr -d '/+=' | cut -c1-32)

keytool -genkeypair -keystore "$work/koan-release.p12" -storetype PKCS12 -alias koan \
    -keyalg RSA -keysize 4096 -validity 10950 \
    -storepass "$password" -keypass "$password" \
    -dname "CN=koan, O=radiosilence"
fingerprint=$(keytool -list -keystore "$work/koan-release.p12" -storepass "$password" -alias koan -v |
    sed -n 's/^[[:space:]]*SHA256: //p')

op document create "$work/koan-release.p12" --title "$TITLE" --file-name koan-release.p12 --vault "$VAULT" >/dev/null
op item create --category password --title "$TITLE password" --vault "$VAULT" \
    "password=$password" "alias[text]=koan" "sha256[text]=$fingerprint" >/dev/null

base64 < "$work/koan-release.p12" | gh secret set ANDROID_RELEASE_KEYSTORE --repo "$REPO"
printf %s "$password" | gh secret set ANDROID_RELEASE_KEYSTORE_PASSWORD --repo "$REPO"

echo "Release key stored in 1Password ($VAULT) as \"$TITLE\", and ANDROID_RELEASE_KEYSTORE{,_PASSWORD} are set on $REPO."
echo "SHA-256: $fingerprint"
