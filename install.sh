#!/usr/bin/env bash

set -euo pipefail

REPO="vendo-analytics/vendo-cli"
API_URL="https://api.github.com/repos/${REPO}"
DOWNLOAD_BASE="https://github.com/${REPO}/releases/download"
INSTALL_DIR="${HOME}/.local/bin"
INSTALL_PATH="${INSTALL_DIR}/vendo"
COMPLETIONS_DIR="${HOME}/.local/share/vendo/completions"
CONFIG_PATH="${HOME}/.config/vendo/config.json"
# Set by install_completions; shown in the summary at the end.
COMPLETIONS_SUMMARY=""

log() {
  printf '%s\n' "$1"
}

fail() {
  printf 'Error: %s\n' "$1" >&2
  exit 1
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    fail "Missing required command: $1"
  fi
}

normalize_tag() {
  case "$1" in
    cli-v*) printf '%s\n' "$1" ;;
    *) printf 'cli-v%s\n' "$1" ;;
  esac
}

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Darwin) os="darwin" ;;
    Linux) os="linux" ;;
    *) fail "Unsupported operating system: $os" ;;
  esac

  case "$arch" in
    x86_64|amd64) arch="x64" ;;
    arm64|aarch64) arch="arm64" ;;
    *) fail "Unsupported architecture: $arch" ;;
  esac

  printf '%s-%s\n' "$os" "$arch"
}

resolve_tag() {
  if [ -n "${VENDO_VERSION:-}" ]; then
    normalize_tag "$VENDO_VERSION"
    return
  fi

  local response tag
  response="$(curl -fsSL -H 'Accept: application/vnd.github+json' "${API_URL}/releases/latest")"
  tag="$(printf '%s' "$response" | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"

  if [ -z "$tag" ]; then
    fail "Unable to resolve the latest Vendo CLI release tag"
  fi

  printf '%s\n' "$tag"
}

verify_checksum() {
  local asset_file checksum_file expected actual
  asset_file="$1"
  checksum_file="$2"

  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$(dirname "$asset_file")" && sha256sum -c "$(basename "$checksum_file")")
    return
  fi

  expected="$(awk '{print $1}' "$checksum_file")"

  if command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$asset_file" | awk '{print $1}')"
  elif command -v openssl >/dev/null 2>&1; then
    actual="$(openssl dgst -sha256 "$asset_file" | awk '{print $NF}')"
  else
    fail "Unable to verify checksum: install sha256sum, shasum, or openssl"
  fi

  if [ "$actual" != "$expected" ]; then
    fail "Checksum verification failed for $(basename "$asset_file")"
  fi
}

install_binary() {
  local asset_file
  asset_file="$1"

  mkdir -p "$INSTALL_DIR"

  if command -v install >/dev/null 2>&1; then
    install -m 0755 "$asset_file" "$INSTALL_PATH"
  else
    cp "$asset_file" "$INSTALL_PATH"
    chmod 0755 "$INSTALL_PATH"
  fi
}

install_completions() {
  if [ "${VENDO_INSTALL_COMPLETIONS:-1}" = "0" ]; then
    COMPLETIONS_SUMMARY="skipped (VENDO_INSTALL_COMPLETIONS=0)"
    return
  fi

  local shell_name login_file
  shell_name="$(basename "${SHELL:-}")"

  case "$shell_name" in
    bash)
      mkdir -p "$COMPLETIONS_DIR"
      "$INSTALL_PATH" completions bash > "${COMPLETIONS_DIR}/vendo.bash"
      ensure_bash_completion_block "${HOME}/.bashrc" "${COMPLETIONS_DIR}/vendo.bash"
      COMPLETIONS_SUMMARY="bash, loaded from $(tilde "${HOME}/.bashrc")"
      # macOS Terminal opens each window as a login shell, which reads its login file, not ~/.bashrc.
      if [ "$(uname -s)" = "Darwin" ]; then
        login_file="$(bash_login_file)"
        ensure_bash_completion_block "$login_file" "${COMPLETIONS_DIR}/vendo.bash"
        COMPLETIONS_SUMMARY="${COMPLETIONS_SUMMARY} and $(tilde "$login_file")"
      fi
      ;;
    zsh)
      mkdir -p "$COMPLETIONS_DIR"
      "$INSTALL_PATH" completions zsh > "${COMPLETIONS_DIR}/vendo.zsh"
      ensure_zsh_completion_block "${HOME}/.zshrc" "${COMPLETIONS_DIR}/vendo.zsh"
      COMPLETIONS_SUMMARY="zsh, loaded from $(tilde "${HOME}/.zshrc")"
      ;;
    fish)
      mkdir -p "${HOME}/.config/fish/completions"
      "$INSTALL_PATH" completions fish > "${HOME}/.config/fish/completions/vendo.fish"
      COMPLETIONS_SUMMARY="fish, in $(tilde "${HOME}/.config/fish/completions/vendo.fish")"
      ;;
    *)
      COMPLETIONS_SUMMARY="skipped (unsupported shell: ${shell_name:-unknown})"
      ;;
  esac
}

# The file a bash login shell reads: the first of these that exists, as bash picks it, else
# ~/.bash_profile, which ensure_bash_completion_block then creates. Never ~/.bash_profile
# beside an existing ~/.profile: bash would stop reading ~/.profile.
bash_login_file() {
  local file
  for file in "${HOME}/.bash_profile" "${HOME}/.bash_login" "${HOME}/.profile"; do
    if [ -e "$file" ]; then
      printf '%s\n' "$file"
      return
    fi
  done
  printf '%s\n' "${HOME}/.bash_profile"
}

ensure_bash_completion_block() {
  local rc_file completion_file
  rc_file="$1"
  completion_file="$2"

  touch "$rc_file"
  if grep -Fq '# >>> vendo completions >>>' "$rc_file"; then
    return
  fi

  cat >> "$rc_file" <<EOF

# >>> vendo completions >>>
[ -f "${completion_file}" ] && source "${completion_file}"
# <<< vendo completions <<<
EOF
}

ensure_zsh_completion_block() {
  local rc_file completion_file
  rc_file="$1"
  completion_file="$2"

  touch "$rc_file"
  if grep -Fq '# >>> vendo completions >>>' "$rc_file"; then
    return
  fi

  cat >> "$rc_file" <<EOF

# >>> vendo completions >>>
if [ -f "${completion_file}" ]; then
  autoload -Uz compinit 2>/dev/null || true
  if ! whence compdef >/dev/null 2>&1; then
    compinit >/dev/null 2>&1
  fi
  source "${completion_file}"
fi
# <<< vendo completions <<<
EOF
}

# Show a path under $HOME as ~/... (display only, so the ~ stays literal).
# shellcheck disable=SC2088
tilde() {
  case "$1" in
    "${HOME}"/*) printf '~/%s' "${1#"${HOME}"/}" ;;
    *) printf '%s' "$1" ;;
  esac
}

print_summary() {
  local version settings
  version="$1"
  if [ -f "$CONFIG_PATH" ]; then
    settings="$(tilde "$CONFIG_PATH") (your existing profiles are kept)"
  else
    settings="$(tilde "$CONFIG_PATH") (created when you sign in)"
  fi

  cat <<'EOF'

                     _
__   _____ _ __   __| | ___
\ \ / / _ \ '_ \ / _` |/ _ \
 \ V /  __/ | | | (_| | (_) |
  \_/ \___|_| |_|\__,_|\___/

EOF
  log "Vendo CLI ${version} is installed."
  log ""
  log "  Command      $(tilde "$INSTALL_PATH")"
  log "  Completions  ${COMPLETIONS_SUMMARY}"
  log "  Settings     ${settings}"
  log ""
  case ":$PATH:" in
    *":${INSTALL_DIR}:"*) log "Get started in a new terminal, so completions load:" ;;
    *) log "Add $(tilde "$INSTALL_DIR") to your PATH, then in a new terminal:" ;;
  esac
  log "  vendo login    Sign in through your browser and check your account"
  log "  vendo --help   See every command"
  log ""
  log "Docs: https://docs.vendodata.com/cli"
}

main() {
  require_command curl
  require_command uname
  require_command mktemp

  local target tag asset_name checksum_name asset_path checksum_path version
  target="$(detect_target)"
  tag="$(resolve_tag)"
  version="${tag#cli-v}"
  asset_name="vendo-${target}"
  checksum_name="${asset_name}.sha256"
  # tmp_dir is intentionally script-scope (not local) so the EXIT trap can
  # see it after main() returns — local vars are out of scope by then,
  # which with `set -u` produces an "unbound variable" error at cleanup.
  tmp_dir="$(mktemp -d)"
  asset_path="${tmp_dir}/${asset_name}"
  checksum_path="${tmp_dir}/${checksum_name}"

  trap 'rm -rf "${tmp_dir:-}"' EXIT

  log "Installing Vendo CLI ${version} for ${target}"
  curl -fsSL -o "$asset_path" "${DOWNLOAD_BASE}/${tag}/${asset_name}"
  curl -fsSL -o "$checksum_path" "${DOWNLOAD_BASE}/${tag}/${checksum_name}"

  verify_checksum "$asset_path" "$checksum_path"
  install_binary "$asset_path"
  install_completions

  print_summary "$version"
}

main "$@"
