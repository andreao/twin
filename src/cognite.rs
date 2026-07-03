//! Cognite CDF fetch — the host side of the **parametrized datapoints boundary lens**
//! (design_doc §9.11 parametrizable lenses, §9.9 boundary adapter).
//!
//! The chart lens is `datapoints(sensorId, window)`: if the requested series isn't
//! materialized locally, this fetches it on demand — anchored to where the sensor's
//! data actually is (its latest datapoint), not a fixed bulk window.  The network
//! effect is a governed host capability (here backed by `curl` for TLS, since raw V8
//! has no network and std has no TLS); the JS lenses stay pure.  The token is the
//! injected credential (§13); an expired one is refreshed via its refresh_token.

use serde_json::Value;
use std::process::Command;

const BASE: &str = "https://api.cognitedata.com/api/v1/projects/publicdata";
const TOKEN_FILE: &str = "/tmp/cognite_token.json";
const TENANT: &str = "48d5043c-cf70-4c49-881c-c638f5796997";
const CLIENT: &str = "1b90ede3-271e-401b-81a0-a4d52bea3273";

fn access_token() -> Result<String, String> {
    let text = std::fs::read_to_string(TOKEN_FILE)
        .map_err(|_| "not signed in — run the obtain-oid skill first".to_string())?;
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v["access_token"].as_str().map(String::from))
        .ok_or_else(|| "bad token file".into())
}

fn curl_post(url: &str, body: &str, tok: &str) -> Result<String, String> {
    let out = Command::new("curl")
        .args([
            "-s", "--max-time", "60",
            "-H", &format!("Authorization: Bearer {tok}"),
            "-H", "Content-Type: application/json",
            "-d", body, url,
        ])
        .output()
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn is_auth_error(resp: &str) -> bool {
    let l = resp.to_lowercase();
    resp.contains("\"code\":401") || l.contains("unauthorized") || l.contains("token") && l.contains("expired")
}

/// Swap the refresh_token for a fresh access token (§13 capability refresh).
fn refresh() -> Result<String, String> {
    let text = std::fs::read_to_string(TOKEN_FILE).map_err(|e| e.to_string())?;
    let rt = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v["refresh_token"].as_str().map(String::from))
        .ok_or("no refresh_token — re-run obtain-oid login")?;
    let url = format!("https://login.microsoftonline.com/{TENANT}/oauth2/v2.0/token");
    let out = Command::new("curl")
        .args([
            "-s", "--max-time", "30",
            "-d", &format!("client_id={CLIENT}"),
            "-d", "grant_type=refresh_token",
            "--data-urlencode", &format!("refresh_token={rt}"),
            "--data-urlencode", "scope=https://api.cognitedata.com/user_impersonation offline_access",
            &url,
        ])
        .output()
        .map_err(|e| e.to_string())?;
    let resp = String::from_utf8_lossy(&out.stdout).into_owned();
    let v: Value = serde_json::from_str(&resp).map_err(|_| "refresh failed".to_string())?;
    let tok = v["access_token"].as_str().ok_or("refresh rejected — re-run obtain-oid login")?;
    let _ = std::fs::write(TOKEN_FILE, &resp); // persist (keeps the new refresh_token)
    Ok(tok.to_string())
}

fn post_with_retry(path: &str, body: &str) -> Result<String, String> {
    let mut tok = access_token()?;
    let url = format!("{BASE}{path}");
    let mut resp = curl_post(&url, body, &tok)?;
    if is_auth_error(&resp) {
        tok = refresh()?;
        resp = curl_post(&url, body, &tok)?;
    }
    Ok(resp)
}

/// Fetch a sensor's raw datapoints, parametrized by id + window (days back from the
/// series' most recent point). Returns the points (possibly empty if truly no data).
pub fn fetch_series(id: &str, days: i64) -> Result<Vec<(i64, f64)>, String> {
    // anchor at where the data actually is
    let latest = post_with_retry("/timeseries/data/latest", &format!(r#"{{"items":[{{"id":{id}}}]}}"#))?;
    let lv: Value = serde_json::from_str(&latest).map_err(|e| format!("bad response: {e}"))?;
    let end = lv["items"][0]["datapoints"][0]["timestamp"]
        .as_i64()
        .ok_or("this sensor has no datapoints at all")?;
    let start = end - days * 86_400_000;
    let body = format!(r#"{{"items":[{{"id":{id}}}],"start":{start},"end":{},"limit":100000}}"#, end + 1);
    let resp = post_with_retry("/timeseries/data/list", &body)?;
    let v: Value = serde_json::from_str(&resp).map_err(|e| format!("bad response: {e}"))?;
    let dps = v["items"][0]["datapoints"]
        .as_array()
        .ok_or("no datapoints in response")?;
    Ok(dps
        .iter()
        .filter_map(|d| Some((d["timestamp"].as_i64()?, d["value"].as_f64()?)))
        .collect())
}
