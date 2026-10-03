#!/bin/sh
# KUVPN host-only split tunnel. The placeholder is replaced with a parsed IPv4 address.
SPLIT_HOST='@SPLIT_HOST@'
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

setup_route() {
    if [ "$OS" = "Darwin" ]; then
        # Reconnect can leave the host route in place. Accept it only if it
        # already belongs to this tunnel; do not replace another interface's route.
        route -n add -host "$SPLIT_HOST" -interface "$TUNDEV" || {
            current_if=$(route -n get "$SPLIT_HOST" | awk '/interface:/{print $2}')
            [ "$current_if" = "$TUNDEV" ]
        }
    else
        ip route replace "$SPLIT_HOST/32" dev "$TUNDEV"
    fi
}

teardown_route() {
    if [ "$OS" = "Darwin" ]; then
        current_if=$(route -n get "$SPLIT_HOST" 2>/dev/null | awk '/interface:/{print $2}')
        if [ "$current_if" = "$TUNDEV" ]; then
            route -n delete -host "$SPLIT_HOST" -interface "$TUNDEV" 2>/dev/null || true
        fi
    else
        ip route del "$SPLIT_HOST/32" dev "$TUNDEV" 2>/dev/null || true
    fi
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
        if [ -z "$TUNDEV" ] || [ -z "$INTERNAL_IP4_ADDRESS" ]; then
            echo "KUVPN split tunnel: missing interface or IPv4 address" >&2
            exit 1
        fi
        if ! setup_interface || ! setup_route; then
            echo "KUVPN split tunnel: failed to configure the host route" >&2
            teardown_interface
            exit 1
        fi
        ;;
    disconnect)
        teardown_route
        teardown_interface
        ;;
    pre-init|attempt-reconnect)
        ;;
esac
