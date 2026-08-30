from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parent.parent


class PackagingBoundaryTests(unittest.TestCase):
    def test_nginx_terminates_tls_to_one_fixed_loopback_upstream(self) -> None:
        config = (ROOT / "packaging/nginx/hardened-blikvm.conf").read_text()
        self.assertIn("ssl_protocols TLSv1.2 TLSv1.3;", config)
        self.assertEqual(config.count("proxy_pass http://127.0.0.1:18080;"), 1)
        self.assertIn(
            "proxy_set_header Host $hardened_blikvm_forwarded_host;", config
        )
        self.assertIn("proxy_set_header Upgrade $http_upgrade;", config)
        self.assertNotIn("proxy_pass http://0.0.0.0", config)
        self.assertNotIn("BEGIN PRIVATE KEY", config)

    def test_server_unit_requires_secure_proxy_cookies(self) -> None:
        unit = (
            ROOT / "packaging/systemd/hardened-blikvm-server.service"
        ).read_text()
        self.assertIn("Environment=BLI_KVM_BIND=127.0.0.1:18080", unit)
        self.assertIn("Environment=BLI_KVM_SECURE_COOKIES=1", unit)
        self.assertIn("IPAddressDeny=any", unit)
        self.assertIn("IPAddressAllow=localhost", unit)
        self.assertIn("SocketBindDeny=any", unit)
        self.assertIn("SocketBindAllow=tcp:18080", unit)

    def test_prototype_builder_includes_templates_but_not_tls_keys(self) -> None:
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        self.assertIn("packaging/nginx/hardened-blikvm.conf", builder)
        self.assertNotIn("tls.key", builder)
        self.assertNotIn("tls.crt", builder)


if __name__ == "__main__":
    _ = unittest.main()
