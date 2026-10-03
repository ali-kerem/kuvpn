"""Exercise the real routing script with fake network commands; no root/VPN needed.

Run with: python3 -m unittest discover -s tests -v
"""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "crates/kuvpn/src/openconnect/split-tunnel.sh"
HOST = "203.0.113.10"
MOCK = r'''
import json
import os
from pathlib import Path
import sys

name = Path(sys.argv[0]).name
args = sys.argv[1:]
if name == "uname":
    print(os.environ["TEST_OS"])
    sys.exit(0)
with open(os.environ["TEST_LOG"], "a") as log:
    log.write(json.dumps([name, *args]) + "\n")
if name in ("scutil", "resolvectl", "networksetup"):
    sys.exit("Split mode must never change DNS or IPv6")
state = Path(os.environ["TEST_ROUTE"])
if name == "route":
    action = args[1]
    if action == "add":
        if state.exists() or os.environ.get("FAIL_ROUTE"):
            sys.exit(1)
        state.write_text(args[-1])
    elif action == "get":
        print("interface: " + (state.read_text() if state.exists() else "en0"))
    elif action == "delete":
        if state.exists() and state.read_text() == args[-1]:
            state.unlink()
elif name == "ip" and args[0] == "route":
    if args[1] == "replace":
        if os.environ.get("FAIL_ROUTE"):
            sys.exit(1)
        state.write_text(args[-1])
    elif args[1] == "del":
        if state.exists() and state.read_text() == args[-1]:
            state.unlink()
'''


class SplitTunnelTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="kuvpn-routing-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.script = self.root / "split.sh"
        self.script.write_text(SCRIPT.read_text().replace("@SPLIT_HOST@", HOST))
        self.log = self.root / "commands.jsonl"
        self.route = self.root / "route-state"
        for name in ["uname", "route", "ip", "ifconfig", "scutil", "resolvectl", "networksetup"]:
            mock = self.root / name
            mock.write_text(f"#!{sys.executable}\n{MOCK}")
            mock.chmod(0o700)
        self.env = {
            "PATH": f"{self.root}:/usr/bin:/bin",
            "TEST_LOG": str(self.log),
            "TEST_ROUTE": str(self.route),
            "TUNDEV": "kuvpn-test0",
            "INTERNAL_IP4_ADDRESS": "10.99.0.8",
            # Adversarial server settings: none should affect split routing.
            "INTERNAL_IP4_NETMASKLEN": "16",
            "INTERNAL_IP4_DNS": "10.99.0.53",
            "CISCO_DEF_DOMAIN": "example.test",
            "CISCO_SPLIT_INC": "1",
            "CISCO_SPLIT_INC_0_ADDR": "0.0.0.0",
            "CISCO_SPLIT_INC_0_MASKLEN": "0",
            "INTERNAL_IP6_ADDRESS": "fd00::8",
            "INTERNAL_IP6_DNS": "fd00::53",
            "VPNGATEWAY": "192.0.2.1",
        }

    def run_script(self, platform, reason, **overrides):
        return subprocess.run(
            ["/bin/sh", str(self.script)],
            env={**self.env, "TEST_OS": platform, "reason": reason, **overrides},
            capture_output=True, text=True, check=False,
        )

    def commands(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def test_posix_shell_syntax(self):
        subprocess.run(["/bin/sh", "-n", str(self.script)], check=True)

    def test_macos_connect_reconnect_disconnect_only_touch_the_host(self):
        for reason in ["connect", "reconnect"]:
            result = self.run_script("Darwin", reason)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.route.read_text(), "kuvpn-test0")
        result = self.run_script("Darwin", "disconnect")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.route.exists())
        self.assertEqual(self.commands(), [
            ["ifconfig", "kuvpn-test0", "10.99.0.8", "10.99.0.8", "mtu", "1400", "netmask", "255.255.255.255", "up"],
            ["route", "-n", "add", "-host", HOST, "-interface", "kuvpn-test0"],
            ["ifconfig", "kuvpn-test0", "10.99.0.8", "10.99.0.8", "mtu", "1400", "netmask", "255.255.255.255", "up"],
            ["route", "-n", "add", "-host", HOST, "-interface", "kuvpn-test0"],
            ["route", "-n", "get", HOST],
            ["route", "-n", "get", HOST],
            ["route", "-n", "delete", "-host", HOST, "-interface", "kuvpn-test0"],
            ["ifconfig", "kuvpn-test0", "down"],
        ])

    def test_linux_connect_reconnect_disconnect_only_touch_the_host(self):
        for reason in ["connect", "reconnect"]:
            result = self.run_script("Linux", reason)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.route.read_text(), "kuvpn-test0")
        result = self.run_script("Linux", "disconnect")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.route.exists())
        self.assertEqual(self.commands(), [
            ["ip", "addr", "replace", "10.99.0.8/32", "dev", "kuvpn-test0"],
            ["ip", "link", "set", "kuvpn-test0", "mtu", "1400", "up"],
            ["ip", "route", "replace", f"{HOST}/32", "dev", "kuvpn-test0"],
        ] * 2 + [
            ["ip", "route", "del", f"{HOST}/32", "dev", "kuvpn-test0"],
            ["ip", "addr", "del", "10.99.0.8/32", "dev", "kuvpn-test0"],
            ["ip", "link", "set", "kuvpn-test0", "down"],
        ])

    def test_macos_conflicting_host_route_is_preserved(self):
        self.route.write_text("en0")
        result = self.run_script("Darwin", "connect")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.route.read_text(), "en0")
        self.assertIn("failed to configure", result.stderr)
        self.assertEqual(self.commands()[-1], ["ifconfig", "kuvpn-test0", "down"])
        self.assertEqual(self.run_script("Darwin", "disconnect").returncode, 0)
        self.assertEqual(self.route.read_text(), "en0")

    def test_failed_route_configuration_does_not_leave_interface_up(self):
        for platform in ["Darwin", "Linux"]:
            with self.subTest(platform=platform):
                result = self.run_script(platform, "connect", FAIL_ROUTE="1")
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(self.route.exists())
                self.assertEqual(self.commands()[-1][-1], "down")

    def test_missing_vpn_address_is_rejected_before_network_changes(self):
        for platform in ["Darwin", "Linux"]:
            result = self.run_script(platform, "connect", INTERNAL_IP4_ADDRESS="")
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(self.commands(), [])

    def test_preinit_and_reconnect_attempt_do_not_change_network(self):
        for platform in ["Darwin", "Linux"]:
            for reason in ["pre-init", "attempt-reconnect"]:
                self.assertEqual(self.run_script(platform, reason).returncode, 0)
        self.assertEqual(self.commands(), [])


if __name__ == "__main__":
    unittest.main()
