#!/usr/bin/env bash
# Quanta / Calagopus Wings node installer
#
#   bash <(curl -sSL https://raw.githubusercontent.com/MrWho1720/Quanta-Rust/main/install.sh)
#
# Env overrides:
#   WINGS_REPO=owner/repo     release source        (default: calagopus/wings)
#   WINGS_VERSION=1.1.0       pin a version         (default: latest)

set -euo pipefail

WINGS_REPO="${WINGS_REPO:-calagopus/wings}"
WINGS_VERSION="${WINGS_VERSION:-latest}"

BINARY=/usr/local/bin/wings
CONFIG_DIR=/etc/pterodactyl
DATA_DIR=/var/lib/pterodactyl

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'
CYAN='\033[0;36m'; BOLD='\033[1m'; DIM='\033[2m'; NC='\033[0m'

# Menus read single keys, so we need a terminal even under `curl ... | bash`.
if [ ! -t 0 ] && [ -e /dev/tty ]; then exec </dev/tty; fi

# ─── Output helpers ──────────────────────────────────────────────────────────

banner() {
    clear
    echo -e "${CYAN}"
    echo "   ██████  ██    ██  █████  ███    ██ ████████  █████ "
    echo "  ██    ██ ██    ██ ██   ██ ████   ██    ██    ██   ██"
    echo "  ██    ██ ██    ██ ███████ ██ ██  ██    ██    ███████"
    echo "  ██ ▄▄ ██ ██    ██ ██   ██ ██  ██ ██    ██    ██   ██"
    echo "   ██████   ██████  ██   ██ ██   ████    ██    ██   ██"
    echo -e "${NC}${DIM}     node daemon — ${WINGS_REPO} @ ${WINGS_VERSION}${NC}"
    echo ""
}

sep()  { echo -e "${BLUE}────────────────────────────────────────────────────────${NC}"; }
step() { echo -e "\n${CYAN}${BOLD}  ▶  $1${NC}"; }
ok()   { echo -e "${GREEN}  ✔  $1${NC}"; }
warn() { echo -e "${YELLOW}  ⚠  $1${NC}"; }
err()  { echo -e "${RED}  ✘  $1${NC}"; }
info() { echo -e "${DIM}     $1${NC}"; }
pause() { echo ""; read -rp "$(echo -e "${DIM}  Press Enter to continue...${NC}")" _; }

# ─── Arrow-key navigation ────────────────────────────────────────────────────

# menu "Title" "item" ...  → sets MENU_CHOICE (0-based). Returns 1 on Esc/q.
menu() {
    local title="$1"; shift
    local items=("$@") sel=0 i key rest
    while true; do
        banner
        sep
        echo -e "  ${BOLD}${title}${NC}"
        sep
        echo ""
        for i in "${!items[@]}"; do
            if [ "$i" -eq "$sel" ]; then
                echo -e "   ${CYAN}${BOLD}❯ ${items[$i]}${NC}"
            else
                echo -e "     ${items[$i]}"
            fi
        done
        echo ""
        echo -e "${DIM}     ↑/↓ move · Enter select · Esc/q back${NC}"

        IFS= read -rsn1 key || return 1
        if [ "$key" = $'\x1b' ]; then
            rest=""
            read -rsn2 -t 0.05 rest || true
            key+="$rest"
        fi
        case "$key" in
            $'\x1b[A'|k) sel=$(( (sel - 1 + ${#items[@]}) % ${#items[@]} )) ;;
            $'\x1b[B'|j) sel=$(( (sel + 1) % ${#items[@]} )) ;;
            ""|$'\n')    MENU_CHOICE=$sel; return 0 ;;
            $'\x1b'|q|Q) return 1 ;;
        esac
    done
}

# confirm "Question?" [y|n]  → 0 = yes, 1 = no
confirm() {
    local q="$1" sel=0 key rest
    [ "${2:-y}" = "n" ] && sel=1
    while true; do
        if [ "$sel" -eq 0 ]; then
            printf "\r  %s   ${GREEN}${BOLD}❯ Yes${NC}     No   " "$q"
        else
            printf "\r  %s     Yes   ${GREEN}${BOLD}❯ No${NC}  " "$q"
        fi
        IFS= read -rsn1 key || { echo ""; return 1; }
        if [ "$key" = $'\x1b' ]; then
            rest=""
            read -rsn2 -t 0.05 rest || true
            key+="$rest"
        fi
        case "$key" in
            $'\x1b[C'|$'\x1b[D'|$'\x1b[A'|$'\x1b[B'|h|l) sel=$((1 - sel)) ;;
            y|Y) sel=0 ;;
            n|N) sel=1 ;;
            ""|$'\n') echo ""; return "$sel" ;;
        esac
    done
}

# ─── System ──────────────────────────────────────────────────────────────────

need_root() {
    if [ "$EUID" -ne 0 ]; then err "Run this installer as root."; exit 1; fi
}

detect_os() {
    if [ ! -f /etc/os-release ]; then err "Unsupported OS (no /etc/os-release)."; exit 1; fi
    # shellcheck disable=SC1091
    . /etc/os-release
    OS="${ID:-unknown}"
}

pkg_install() {
    case "$OS" in
        ubuntu|debian) apt-get update -y -qq && apt-get install -y -qq "$@" ;;
        centos|rhel|almalinux|rocky|fedora) dnf install -y "$@" ;;
        *) warn "Unknown distro '$OS' — install manually: $*" ;;
    esac
}

ensure_docker() {
    if docker info &>/dev/null; then ok "Docker ready."; return; fi
    step "Installing Docker..."
    curl -sSL https://get.docker.com/ | CHANNEL=stable bash
    systemctl enable --now docker || { err "Docker failed to start."; exit 1; }
    ok "Docker ready."
}

asset_url() {
    local arch name
    arch="$(uname -m)"
    case "$arch" in
        x86_64|aarch64|riscv64|ppc64le) ;;
        *) err "Unsupported architecture: $arch"; return 1 ;;
    esac
    name="wings-rs-${arch}-linux"
    if [ "$WINGS_VERSION" = "latest" ]; then
        echo "https://github.com/${WINGS_REPO}/releases/latest/download/${name}"
    else
        echo "https://github.com/${WINGS_REPO}/releases/download/release-${WINGS_VERSION#release-}/${name}"
    fi
}

# Download to a temp file first: curl -o onto a running binary fails with ETXTBSY,
# a rename over it does not.
download_binary() {
    local url tmp
    url="$(asset_url)" || return 1
    tmp="$(mktemp /tmp/wings.XXXXXX)"
    step "Downloading $url"
    if ! curl -fL --progress-bar -o "$tmp" "$url"; then
        rm -f "$tmp"
        err "Download failed — check that $WINGS_REPO has a $WINGS_VERSION release for $(uname -m)."
        return 1
    fi
    chmod +x "$tmp"
    mv -f "$tmp" "$BINARY"
    ok "Installed $BINARY ($("$BINARY" version 2>/dev/null | head -1))"
}

configure_join_data() {
    echo ""
    info "Paste the join data from the panel (blank to skip and configure later)."
    read -rp "  Join data: " JOIN_DATA
    if [ -z "$JOIN_DATA" ]; then
        warn "Skipped — configure later with: wings configure --join-data <blob>"
        return 1
    fi
    "$BINARY" configure --override --join-data "$JOIN_DATA" || { err "Configure failed."; return 1; }
    ok "Configuration written to $CONFIG_DIR/config.yml"
}

request_cert() {
    local domain
    echo ""
    read -rp "  Node domain (e.g. node.example.com, blank to skip): " domain
    [ -z "$domain" ] && { warn "Skipped SSL."; return 0; }

    step "Installing Certbot..."
    pkg_install certbot iproute2

    step "Requesting certificate for $domain..."
    certbot certonly --standalone -d "$domain" --non-interactive --agree-tos \
        -m "admin@${domain}" || { warn "Certbot failed — run it manually."; return 0; }

    if [ -f "/etc/letsencrypt/live/${domain}/fullchain.pem" ]; then
        ok "Certificate issued."
        info "cert: /etc/letsencrypt/live/${domain}/fullchain.pem"
        info "key:  /etc/letsencrypt/live/${domain}/privkey.pem"
        info "Point api.ssl.cert / api.ssl.key in $CONFIG_DIR/config.yml at those files."
    fi
}

# ─── Actions ─────────────────────────────────────────────────────────────────

action_install() {
    banner
    step "1/5 — System packages"
    pkg_install curl tar ca-certificates
    ok "Packages ready."

    step "2/5 — Docker"
    ensure_docker

    step "3/5 — Wings binary"
    mkdir -p "$CONFIG_DIR" "$DATA_DIR/volumes"
    download_binary || { pause; return; }

    step "4/5 — Panel configuration"
    configure_join_data || true

    step "5/5 — Systemd service"
    # service-install enables the unit, and starts it too when a config exists.
    "$BINARY" service-install --override || warn "Service install failed — run 'wings service-install' manually."

    if command -v ufw &>/dev/null; then
        ufw allow 8080/tcp >/dev/null 2>&1 || true
        ufw allow 2022/tcp >/dev/null 2>&1 || true
        ok "Firewall rules added (8080, 2022)."
    fi

    echo ""
    if confirm "Request an SSL certificate now?" y; then request_cert; fi

    echo ""
    sep
    ok "Wings installed."
    info "status: systemctl status wings"
    info "logs:   journalctl -fu wings"
    sep
    pause
}

action_update() {
    banner
    if [ ! -x "$BINARY" ]; then err "Wings is not installed."; pause; return; fi

    local running=false
    if systemctl is-active --quiet wings; then
        running=true
        step "Stopping wings..."
        systemctl stop wings
    fi

    download_binary || { $running && systemctl start wings || true; pause; return; }

    if $running; then
        systemctl start wings
        ok "Wings restarted."
    else
        info "Start it with: systemctl start wings"
    fi
    pause
}

action_configure() {
    banner
    if [ ! -x "$BINARY" ]; then err "Wings is not installed."; pause; return; fi
    if configure_join_data; then
        systemctl restart wings 2>/dev/null || true
        ok "Wings restarted with the new configuration."
    fi
    pause
}

action_ssl() {
    banner
    request_cert
    pause
}

action_uninstall() {
    banner
    warn "This removes the Wings binary and its service."
    echo ""
    read -rp "  Type 'yes' to confirm: " c
    [ "$c" = "yes" ] || { info "Aborted."; pause; return; }

    systemctl stop wings 2>/dev/null || true
    systemctl disable wings 2>/dev/null || true
    rm -f /etc/systemd/system/wings.service
    systemctl daemon-reload
    rm -f "$BINARY"
    ok "Binary and service removed."

    echo ""
    read -rp "  Purge ALL node data ($CONFIG_DIR, $DATA_DIR)? Type 'purge' to confirm: " p
    if [ "$p" = "purge" ]; then
        rm -rf "$CONFIG_DIR" "$DATA_DIR"
        ok "Node data purged."
    else
        info "Data preserved at $CONFIG_DIR and $DATA_DIR."
    fi
    pause
}

# ─── Main ────────────────────────────────────────────────────────────────────

need_root
detect_os

while menu "Wings — node daemon" \
    "Install" \
    "Update binary" \
    "Configure (join data)" \
    "SSL certificate" \
    "Uninstall" \
    "Exit"; do
    case "$MENU_CHOICE" in
        0) action_install ;;
        1) action_update ;;
        2) action_configure ;;
        3) action_ssl ;;
        4) action_uninstall ;;
        5) exit 0 ;;
    esac
done
