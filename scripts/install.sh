#!/bin/sh
set -eu

REPOSITORY="${PURRCODE_REPOSITORY:-Weilin0723/PurrCode}"
VERSION="${PURRCODE_VERSION:-v0.5.0}"
INSTALL_DIR="${PURRCODE_INSTALL_DIR:-$HOME/.local/bin}"
BASE_URL="${PURRCODE_BASE_URL:-https://github.com/${REPOSITORY}/releases/download/${VERSION}}"

case "$(uname -s)" in
    Darwin) os="apple-darwin" ;;
    Linux) os="unknown-linux-gnu" ;;
    *)
        echo "PurrCode installer supports macOS and Linux; use the Windows release archive on Windows." >&2
        exit 1
        ;;
esac

case "$(uname -m)" in
    x86_64|amd64) arch="x86_64" ;;
    arm64|aarch64) arch="aarch64" ;;
    *)
        echo "Unsupported CPU architecture: $(uname -m)" >&2
        exit 1
        ;;
esac

target="${arch}-${os}"
archive="purrcode-${target}.tar.gz"
temporary="$(mktemp -d "${TMPDIR:-/tmp}/purrcode-install.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

download() {
    source_url="$1"
    destination="$2"
    if command -v curl >/dev/null 2>&1; then
        curl --fail --location --silent --show-error "$source_url" --output "$destination"
    elif command -v wget >/dev/null 2>&1; then
        wget --quiet "$source_url" --output-document="$destination"
    else
        echo "Install curl or wget and retry." >&2
        exit 1
    fi
}

download "${BASE_URL}/${archive}" "${temporary}/${archive}"
download "${BASE_URL}/SHA256SUMS" "${temporary}/SHA256SUMS"

expected="$(awk -v name="$archive" '$2 == name { print $1 }' "${temporary}/SHA256SUMS")"
if [ -z "$expected" ]; then
    echo "Release checksum manifest does not contain ${archive}." >&2
    exit 1
fi

if command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "${temporary}/${archive}" | awk '{ print $1 }')"
elif command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "${temporary}/${archive}" | awk '{ print $1 }')"
else
    echo "A SHA-256 tool (shasum or sha256sum) is required." >&2
    exit 1
fi

if [ "$expected" != "$actual" ]; then
    echo "Checksum verification failed for ${archive}." >&2
    exit 1
fi

tar -xzf "${temporary}/${archive}" -C "$temporary"
mkdir -p "$INSTALL_DIR"
install -m 755 "${temporary}/purrcode-${target}/purrcode" "$INSTALL_DIR/purrcode"
install -m 755 "${temporary}/purrcode-${target}/purrcoded" "$INSTALL_DIR/purrcoded"

echo "Installed PurrCode ${VERSION} to ${INSTALL_DIR}."
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *) echo "Add ${INSTALL_DIR} to PATH, then run: purrcode init" ;;
esac
