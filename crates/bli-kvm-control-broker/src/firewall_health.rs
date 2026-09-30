//! Bounded, certificate-verified HTTPS preflight against the local front only.
use crate::firewall_settings::FirewallRunner;
pub const CURL: &str = "/usr/bin/curl";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpsProbe {
    pub server_name: String,
    pub ca_certificate: String,
}

impl HttpsProbe {
    pub fn validate(&self) -> Result<(), String> {
        let name = &self.server_name;
        if name.is_empty()
            || name.len() > 253
            || name.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || !label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                    || label.starts_with('-')
                    || label.ends_with('-')
            })
        {
            return Err("invalid HTTPS probe server name".into());
        }
        if self.ca_certificate.len() > 4096
            || self.ca_certificate.contains('\0')
            || !std::path::Path::new(&self.ca_certificate).is_absolute()
        {
            return Err("invalid HTTPS probe CA path".into());
        }
        Ok(())
    }

    pub fn check(&self, runner: &mut dyn FirewallRunner, port: u16) -> Result<(), String> {
        self.validate()?;
        if port == 0 {
            return Err("invalid HTTPS probe port".into());
        }
        let resolve = format!("{}:{port}:127.0.0.1", self.server_name);
        let url = format!("https://{}:{port}/api/health", self.server_name);
        let output = runner.run(
            CURL,
            &[
                "--disable",
                "--silent",
                "--show-error",
                "--fail",
                "--noproxy",
                "*",
                "--proto",
                "=https",
                "--connect-timeout",
                "2",
                "--max-time",
                "3",
                "--cacert",
                &self.ca_certificate,
                "--resolve",
                &resolve,
                "--url",
                &url,
                "--write-out",
                "\n%{http_code}",
            ],
        )?;
        if output.len() > 4096 {
            return Err("oversized HTTPS health response".into());
        }
        let (body, status) = output.rsplit_once('\n').ok_or("missing HTTPS status")?;
        if status != "200" {
            return Err("HTTPS health did not return HTTP 200".into());
        }
        let value: serde_json::Value =
            serde_json::from_str(body).map_err(|_| "invalid HTTPS health response")?;
        if value["code"] != 0
            || value["data"]["status"] != "ok"
            || value["data"]["backend"] != "rust"
            || value["data"]["platformId"] != "bli-h616-pikvm"
        {
            return Err("HTTPS front health check failed".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Runner(String);
    impl FirewallRunner for Runner {
        fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
            assert_eq!(program, CURL);
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--resolve", "fixture.local:8443:127.0.0.1"])
            );
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--cacert", "/fixture/ca.crt"])
            );
            assert!(!args.contains(&"--insecure"));
            assert!(!args.contains(&"--location"));
            Ok(self.0.clone())
        }
        fn exists(&mut self, _: &std::path::Path) -> bool {
            false
        }
        fn read_file(&mut self, _: &std::path::Path) -> Result<String, String> {
            panic!("unexpected file access")
        }
        fn write_file(&mut self, _: &std::path::Path, _: &str) -> Result<(), String> {
            panic!("unexpected write")
        }
        fn remove_file(&mut self, _: &std::path::Path) -> Result<(), String> {
            panic!("unexpected removal")
        }
    }
    #[test]
    fn only_successful_native_health_is_accepted() {
        let probe = HttpsProbe {
            server_name: "fixture.local".into(),
            ca_certificate: "/fixture/ca.crt".into(),
        };
        let valid = concat!(
            r#"{"code":0,"data":{"status":"ok","backend":"rust","platformId":"bli-h616-pikvm"}}"#,
            "\n200"
        );
        assert!(probe.check(&mut Runner(valid.into()), 8443).is_ok());
        for invalid in [
            "{}",
            "not JSON",
            &valid.replace("\"code\":0", "\"code\":1"),
            &valid.replace("\"ok\"", "\"failed\""),
            &valid.replace("bli-h616-pikvm", "other"),
        ] {
            assert!(probe.check(&mut Runner(invalid.into()), 8443).is_err());
        }
    }
    #[test]
    fn probe_cannot_target_a_url_or_inject_options() {
        for name in [
            "-k",
            "x/secret",
            "x:443",
            "x@remote",
            "x\n--insecure",
            "a..b",
        ] {
            assert!(
                HttpsProbe {
                    server_name: name.into(),
                    ca_certificate: "/fixture/ca.crt".into()
                }
                .validate()
                .is_err()
            );
        }
    }
}
