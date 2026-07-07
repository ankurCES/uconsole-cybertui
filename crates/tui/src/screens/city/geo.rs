//! IP → CityLocation via ip-api.com.
//!
//! Step 5 builds out the real client. This stub exposes the same
//! type signatures so Steps 3-4 compile in isolation.
//!
//! See: <https://ip-api.com/docs/api:json> — free, no key, HTTP only
//! (the free tier doesn't allow HTTPS), 45 req/min per source IP.
//! We hit it exactly once on City-screen focus + on manual override.
//!
//! The free endpoint returns a flat JSON object; the relevant fields
//! are surfaced in `CityLocation` below. We deliberately don't try to
//! pull street/postal/ISP data — only what's needed to render a city.

use std::time::Duration;

use serde::Deserialize;

/// Re-export the canonical `CityLocation` from `cyberdeck-core` so the
/// TUI renderer, the CLI, and the (future) web dashboard share one
/// type. The HTTP client in Step 5 deserializes ip-api's payload
/// straight into this struct via `serde_json::from_value`.
pub use cyberdeck_core::city::CityLocation;

/// Locator errors. Mapped to user-facing toasts by `CityScreen`.
#[derive(Debug, thiserror::Error)]
pub enum GeoError {
    #[error("network: {0}")]
    Network(#[from] reqwest::Error),
    #[error("rate-limited by ip-api (try again later)")]
    RateLimited,
    #[error("ip-api returned an unexpected payload: {0}")]
    BadPayload(String),
}

/// ip-api.com endpoint. Free tier is HTTP-only; we don't try HTTPS.
const IP_API_URL: &str = "http://ip-api.com/json/?fields=status,country,countryCode,region,regionName,city,zip,lat,lon,timezone,query";

/// User-Agent sent on every request. ip-api logs the UA on their
/// dashboard so a real identifier helps when the user is debugging a
/// `rate-limited` toast.
const USER_AGENT: &str = concat!("cyberdeck-tui/", env!("CARGO_PKG_VERSION"));

/// Request timeout. ip-api typically responds in <500ms; 5s is
/// generous headroom for the free tier's slow path.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Internal ip-api response shape. We deserialize into this, then
/// validate `status == "success"` and project into `CityLocation`.
/// Fields not in the public `CityLocation` (zip, regionName, query)
/// are kept here for future fields without having to touch the wire
/// shape.
#[derive(Debug, Deserialize)]
struct IpApiResponse {
    status: String,
    city: Option<String>,
    country: Option<String>,
    #[serde(rename = "countryCode")]
    country_code: Option<String>,
    #[serde(rename = "regionName")]
    region_name: Option<String>,
    lat: Option<f64>,
    lon: Option<f64>,
    timezone: Option<String>,
}

impl IpApiResponse {
    fn into_location(self) -> Result<CityLocation, GeoError> {
        if self.status != "success" {
            // ip-api returns `status: "fail"` with a `message` field
            // for rate-limit / private-IP / etc. We only have the
            // status field in our request so we surface a generic
            // message; the most common cause in production is
            // rate-limit (HTTP 200 with status:fail).
            return Err(GeoError::RateLimited);
        }
        let lat = self.lat.ok_or_else(|| {
            GeoError::BadPayload("missing lat".into())
        })?;
        let lon = self.lon.ok_or_else(|| {
            GeoError::BadPayload("missing lon".into())
        })?;
        Ok(CityLocation {
            name: self.city.unwrap_or_else(|| "(unknown)".into()),
            country: self.country.unwrap_or_default(),
            country_code: self.country_code.unwrap_or_default(),
            region: self.region_name.unwrap_or_default(),
            lat,
            lon,
            // ip-api's free tier doesn't return a bbox; the roads
            // loader derives one from the bundled polylines instead.
            bbox: None,
            timezone: self.timezone.unwrap_or_default(),
        })
    }
}

/// Fetch the user's location from their public IP. One-shot —
/// callers are expected to debounce (we hit ip-api on City focus,
/// on manual override, and never again until the next focus).
///
/// Errors are non-fatal: the caller logs a warn toast and falls
/// back to the bundled city list (`CityRoads::BUNDLED`).
pub async fn locate() -> Result<CityLocation, GeoError> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .build()?;
    let resp: IpApiResponse = client.get(IP_API_URL).send().await?.json().await?;
    resp.into_location()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Wire shape: ip-api returns `status: "success"` with the
    /// documented fields. We must project it into `CityLocation`
    /// with all optional fields filled where supplied.
    #[tokio::test]
    async fn locate_parses_success_payload() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/json/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{
                    "status": "success",
                    "city": "Seattle",
                    "country": "United States",
                    "countryCode": "US",
                    "regionName": "Washington",
                    "zip": "98101",
                    "lat": 47.6062,
                    "lon": -122.3321,
                    "timezone": "America/Los_Angeles",
                    "query": "1.2.3.4"
                }"#,
            ))
            .mount(&server)
            .await;

        let client = reqwest::Client::builder().build().unwrap();
        let resp: IpApiResponse = client
            .get(format!("{}/json/?fields=status,lat,lon", server.uri()))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let loc = resp.into_location().expect("success → location");
        assert_eq!(loc.name, "Seattle");
        assert_eq!(loc.country_code, "US");
        assert_eq!(loc.region, "Washington");
        assert!((loc.lat - 47.6062).abs() < 1e-6);
        assert!((loc.lon - -122.3321).abs() < 1e-6);
        assert_eq!(loc.timezone, "America/Los_Angeles");
        assert!(loc.bbox.is_none(), "free tier doesn't return bbox");
    }

    /// Wire shape: ip-api returns `status: "fail"` for rate-limit
    /// / private-IP / etc. We must surface that as `RateLimited`
    /// so the renderer can show "ip-api rate-limited (try later)"
    /// rather than a generic network error.
    #[tokio::test]
    async fn locate_rate_limit_maps_to_rate_limited() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/json/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{
                    "status": "fail",
                    "message": "quota exceeded",
                    "query": "1.2.3.4"
                }"#,
            ))
            .mount(&server)
            .await;

        let client = reqwest::Client::builder().build().unwrap();
        let resp: IpApiResponse = client
            .get(format!("{}/json/?fields=status", server.uri()))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        match resp.into_location() {
            Err(GeoError::RateLimited) => {}
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    /// Defensive: a 200 with `status: "success"` but missing lat/lon
    /// must surface as `BadPayload`, not panic. (Real ip-api never
    /// does this, but a future API change shouldn't crash us.)
    #[tokio::test]
    async fn locate_missing_lat_maps_to_bad_payload() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/json/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{
                    "status": "success",
                    "city": "Nowhere"
                }"#,
            ))
            .mount(&server)
            .await;

        let client = reqwest::Client::builder().build().unwrap();
        let resp: IpApiResponse = client
            .get(format!("{}/json/?fields=status,city", server.uri()))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(matches!(resp.into_location(), Err(GeoError::BadPayload(_))));
    }
}