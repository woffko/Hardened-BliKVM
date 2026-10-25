#!/bin/sh
set -eu

temp_list=$(mktemp /tmp/hardened-blikvm-source-list.XXXXXX)
cleanup() {
  case "$temp_list" in
    /tmp/hardened-blikvm-source-list.*) rm -f -- "$temp_list" ;;
    *) echo "refusing unsafe temporary cleanup" >&2 ;;
  esac
}
trap cleanup EXIT HUP INT TERM

git ls-files --cached --others --exclude-standard | sort > "$temp_list"
if [ ! -s "$temp_list" ]; then
  echo "no Git-visible source files found" >&2
  exit 1
fi

while IFS= read -r path; do
  case "$path" in
    evidence/private/*|.codex/*|.agents/*|AGENTS.override.md|web/dist/*|target/*|*/node_modules/*|state/*|scratch/*|vendor/*|*.7z|*.img|*.raw|*.deb)
      echo "forbidden Git-visible path: $path" >&2
      exit 1
      ;;
  esac
  if [ -L "$path" ]; then
    echo "unexpected source symlink: $path" >&2
    exit 1
  fi
  size=$(wc -c < "$path" | tr -d ' ')
  if [ "$size" -gt 2097152 ]; then
    echo "unexpected source file larger than 2 MiB: $path" >&2
    exit 1
  fi
  mime=$(file -b --mime-type "$path")
  case "$mime" in
    text/*|application/json|application/toml|application/xml|application/javascript|inode/x-empty) ;;
    image/png)
      # The logo, and the documentation's screenshots (docs/screenshots.md):
      # made with the built page from recorded, anonymized API answers and a
      # drawn stand-in for the target's screen, each one looked at before it
      # was added.
      case "$path" in
        web/public/hardened-blikvm-logo.png) ;;
        docs/screenshots/*/*)
          echo "unexpected PNG source asset: $path" >&2
          exit 1
          ;;
        docs/screenshots/[0-9][0-9]-*.png|docs/screenshots/[0-9][0-9][a-z]-*.png) ;;
        *)
          echo "unexpected PNG source asset: $path" >&2
          exit 1
          ;;
      esac
      ;;
    image/jpeg)
      # Synthetic test frames for the video encoder's decoder tests: ffmpeg
      # testsrc2, and one generated pattern in four subsamplings
      # (crates/bli-kvm-video-encoder/tests/fixtures/README.md).
      case "$path" in
        crates/bli-kvm-video-encoder/tests/fixtures/testsrc-64x48-420.jpg|crates/bli-kvm-video-encoder/tests/fixtures/testsrc-64x48-422.jpg) ;;
        crates/bli-kvm-video-encoder/tests/fixtures/pattern-320x180-444.jpg|crates/bli-kvm-video-encoder/tests/fixtures/pattern-320x180-422.jpg) ;;
        crates/bli-kvm-video-encoder/tests/fixtures/pattern-320x180-420.jpg|crates/bli-kvm-video-encoder/tests/fixtures/pattern-320x180-gray.jpg) ;;
        *)
          echo "unexpected JPEG source asset: $path" >&2
          exit 1
          ;;
      esac
      ;;
    application/pgp-keys)
      # Tailscale's public apt repository key, fingerprint
      # 2596A99EAAB33821893C0A79458CA832957F5868.
      case "$path" in
        packaging/tailscale/tailscale-archive-keyring.asc) ;;
        *)
          echo "unexpected key file: $path" >&2
          exit 1
          ;;
      esac
      ;;
    image/vnd.microsoft.icon)
      # The target agent's icon, made from the logo above by
      # tools/build_agent_icon.mjs.
      if [ "$path" != "crates/bli-kvm-target-agent/assets/agent.ico" ]; then
        echo "unexpected icon source asset: $path" >&2
        exit 1
      fi
      ;;
    image/svg+xml)
      case "$path" in
        *.svg) ;;
        *)
          echo "unexpected SVG source asset path: $path" >&2
          exit 1
          ;;
      esac
      ;;
    *)
      echo "unexpected binary source file ($mime): $path" >&2
      exit 1
      ;;
  esac
done < "$temp_list"

secret_matches=$(
  rg --hidden -n -i \
    --glob '!.git/**' \
    --glob '!.codex/**' \
    --glob '!AGENTS.override.md' \
    --glob '!evidence/private/**' \
    --glob '!web/public/*.png' \
    "BEGIN [A-Z ]*PRIVATE KEY|github_pat_[A-Za-z0-9_]{20,}|gh[pousr]_[A-Za-z0-9]{30,}|AKIA[0-9A-Z]{16}|(api[_-]?key|token|secret)[[:space:]]*[:=][[:space:]]*[\"'][A-Za-z0-9+/=_-]{16,}[\"']|https?://[^/[:space:]@:]+:[^@[:space:]]+@" \
    . || true
)
secret_matches=$(printf '%s\n' "$secret_matches" | rg -v '^\./tools/test_' || true)
if [ -n "$secret_matches" ]; then
  printf '%s\n' "$secret_matches" >&2
  echo "potential secret in Git-visible source" >&2
  exit 1
fi

private_ip_matches=$(
  rg --hidden -n \
    --glob '!.git/**' \
    --glob '!.codex/**' \
    --glob '!AGENTS.override.md' \
    --glob '!evidence/private/**' \
    --glob '!web/public/*.png' \
    '\b(10\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}|192\.168\.[0-9]{1,3}\.[0-9]{1,3}|172\.(1[6-9]|2[0-9]|3[01])\.[0-9]{1,3}\.[0-9]{1,3})\b' \
    . || true
)
private_ip_matches=$(printf '%s\n' "$private_ip_matches" \
  | rg -v '^\./tools/test_capture_pikvm_baseline\.py:' || true)
# These complete, path-bound lines define the public RFC1918 ranges, not a
# device address. Anchor the entire line so another literal cannot hide beside
# an allowed definition. Do not exempt either firewall source file as a whole.
private_ip_matches=$(printf '%s\n' "$private_ip_matches" \
  | rg -v '^\./crates/bli-kvm-control-broker/src/firewall_rules\.rs:[0-9]+:const LOCAL_V4: &str = "\{ 10[.]0[.]0[.]0/8, 172[.]16[.]0[.]0/12, 192[.]168[.]0[.]0/16, 169[.]254[.]0[.]0/16 \}";$' \
  | rg -v '^\./crates/bli-kvm-control-broker/src/firewall_observation\.rs:[0-9]+:    json!\(\{"set":\[\{"prefix":\{"addr":"10[.]0[.]0[.]0","len":8\}\}, \{"prefix":\{"addr":"172[.]16[.]0[.]0","len":12\}\}, \{"prefix":\{"addr":"192[.]168[.]0[.]0","len":16\}\}, \{"prefix":\{"addr":"169[.]254[.]0[.]0","len":16\}\}\]\}\)$' || true)
if [ -n "$private_ip_matches" ]; then
  printf '%s\n' "$private_ip_matches" >&2
  echo "private IPv4 literal in Git-visible source" >&2
  exit 1
fi

echo "source boundary check passed"
