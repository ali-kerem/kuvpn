#!/bin/sh
# KUVPN host-only split tunnel. Destinations are parsed IPv4 addresses, space-separated.
SPLIT_HOSTS='@SPLIT_HOSTS@'
OS="$(uname -s)"

# Deliberately ignore server-provided routes, DNS, domains, and IPv6 settings.
# A /32 interface address also prevents Linux from adding a route for an entire subnet.
setup_interface() {
    if [ "$OS" = "Darwin" ]; then
        ifconfig "$TUNDEV" "$INTERNAL_IP4_ADDRESS" "$INTERNAL_IP4_ADDRESS" \
            mtu "${INTERNAL_IP4_MTU:-1400}" netmask 255.255.255.255 up
    else
        ip addr replace "$INTERNAL_IP4_ADDRESS/32" dev "$TUNDEV" &&
            ip link set "$TUNDEV" mtu "${INTERNAL_IP4_MTU:-1400}" up
    fi
}

setup_routes() {
    for split_host in $SPLIT_HOSTS; do
        if [ "$OS" = "Darwin" ]; then
            # Reconnect can leave host routes in place. Accept only this tunnel's routes.
            route -n add -host "$split_host" -interface "$TUNDEV" || {
                current_if=$(route -n get "$split_host" | awk '/interface:/{print $2}')
                [ "$current_if" = "$TUNDEV" ] || return 1
            }
        else
            ip route replace "$split_host/32" dev "$TUNDEV" || return 1
        fi
    done
}

teardown_routes() {
    for split_host in $SPLIT_HOSTS; do
        if [ "$OS" = "Darwin" ]; then
            current_if=$(route -n get "$split_host" 2>/dev/null | awk '/interface:/{print $2}')
            if [ "$current_if" = "$TUNDEV" ]; then
                route -n delete -host "$split_host" -interface "$TUNDEV" 2>/dev/null || true
            fi
        else
            ip route del "$split_host/32" dev "$TUNDEV" 2>/dev/null || true
        fi
    done
}

teardown_interface() {
    if [ "$OS" = "Darwin" ]; then
        ifconfig "$TUNDEV" down 2>/dev/null || true
    else
        ip addr del "$INTERNAL_IP4_ADDRESS/32" dev "$TUNDEV" 2>/dev/null || true
        ip link set "$TUNDEV" down 2>/dev/null || true
    fi
}

case "$reason" in
    connect|reconnect)
        if [ -z "$TUNDEV" ] || [ -z "$INTERNAL_IP4_ADDRESS" ] || [ -z "$SPLIT_HOSTS" ]; then
            echo "KUVPN split tunnel: missing interface, VPN IPv4 address, or destinations" >&2
            exit 1
        fi
        if ! setup_interface || ! setup_routes; then
            echo "KUVPN split tunnel: failed to configure host routes" >&2
            teardown_routes
            teardown_interface
            exit 1
        fi
        ;;
    disconnect)
        teardown_routes
        teardown_interface
        ;;
    pre-init|attempt-reconnect)
        ;;
esac
