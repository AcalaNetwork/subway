use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

pub trait XFF {
    fn xxf_ip(&self) -> Option<String>;
}
impl<T> XFF for http::Request<T> {
    fn xxf_ip(&self) -> Option<String> {
        let xff = self.headers().get("x-forwarded-for")?;
        let xff = xff.to_str().ok()?;
        let xff = xff.split(',').next()?;
        let addr = IpAddr::from_str(xff)
            .ok()
            .or(SocketAddr::from_str(xff).map(|x| x.ip()).ok())?;
        Some(addr.to_string())
    }
}

#[test]
fn test_xff() {
    let cases = vec![
        ("", None),
        ("foo,bar", None),
        ("1.2.3.4:1234,foo,bar", Some("1.2.3.4")),
        ("203.0.113.195, 70.41.3.18, 150.172.238.178", Some("203.0.113.195")),
        ("203.0.113.195", Some("203.0.113.195")),
        ("[::1]:1234,foo,bar", Some("::1")),
        (
            "2001:db8:85a3:8d3:1319:8a2e:370:7348",
            Some("2001:db8:85a3:8d3:1319:8a2e:370:7348"),
        ),
        (
            "[2001:db8::1a2b:3c4d]:41237, 198.51.100.100:26321",
            Some("2001:db8::1a2b:3c4d"),
        ),
    ];

    for (xff, ip) in cases {
        let req = http::Request::builder()
            .header("X-Forwarded-For", xff)
            .body(())
            .unwrap();
        assert_eq!(req.xxf_ip().as_deref(), ip);
    }
}
