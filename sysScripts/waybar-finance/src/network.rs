use crate::app::Config;
use crate::app::{MarketStatus, StockDetails};
use crate::config::{MAX_SIDEBAR_QUOTES, normalize_symbol};
use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use tokio::sync::Mutex;

const MAX_SEARCH_QUERY_LENGTH: usize = 64;
const MAX_SEARCH_RESULTS: usize = 12;
const MAX_CONCURRENT_QUOTES: usize = 4;
const CRUMB_FAILURE_COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct MarketQuote {
    pub price: f64,
    pub percent: f64,
}

#[derive(Debug, Serialize)]
pub struct WaybarOutput {
    pub text: String,
    pub tooltip: String,
    pub class: String,
}

#[derive(Debug, Deserialize)]
pub struct YahooSearchResponse {
    pub quotes: Vec<YahooSearchResult>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct YahooSearchResult {
    pub symbol: String,

    #[serde(rename = "shortname")]
    pub name: Option<String>,

    #[serde(rename = "quoteType")]
    pub quote_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct YahooQuoteResponse {
    #[serde(rename = "quoteResponse")]
    quote_response: QuoteResult,
}

#[derive(Debug, Deserialize)]
struct QuoteResult {
    result: Vec<YahooQuote>,
}

#[derive(Debug, Deserialize)]
struct YahooQuote {
    #[serde(rename = "marketCap")]
    market_cap: Option<f64>,

    #[serde(rename = "netAssets")]
    net_assets: Option<f64>,

    #[serde(rename = "trailingPE")]
    pe_ratio: Option<f64>,

    #[serde(rename = "dividendYield")]
    dividend_yield: Option<f64>,

    #[serde(rename = "trailingAnnualDividendYield")]
    trailing_yield: Option<f64>,

    #[serde(rename = "fiftyTwoWeekHigh")]
    high_52w: Option<f64>,

    #[serde(rename = "fiftyTwoWeekLow")]
    low_52w: Option<f64>,
    #[serde(rename = "regularMarketPrice")]
    regular_market_price: Option<f64>,

    #[serde(rename = "ytdReturn")]
    ytd_return: Option<f64>,

    #[serde(rename = "fiftyTwoWeekChangePercent")]
    fifty_two_week_change: Option<f64>,

    symbol: String,
}

#[derive(Debug, Deserialize)]
struct YahooChartResponse {
    chart: YahooChart,
}

#[derive(Debug, Deserialize)]
struct YahooChart {
    result: Option<Vec<YahooChartResult>>,
    error: Option<YahooChartError>,
}

#[derive(Debug, Deserialize)]
struct YahooChartError {
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct YahooChartResult {
    meta: YahooChartMeta,
    timestamp: Option<Vec<i64>>,
    indicators: YahooChartIndicators,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct YahooChartMeta {
    regular_market_price: Option<f64>,
    chart_previous_close: Option<f64>,
    previous_close: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct YahooChartIndicators {
    quote: Vec<YahooChartQuote>,
}

#[derive(Debug, Deserialize)]
struct YahooChartQuote {
    close: Vec<Option<f64>>,
}
#[derive(Debug, Clone)]
struct CachedCrumb {
    value: String,
    generation: u64,
}

#[derive(Debug)]
struct CrumbRefreshFailure {
    observed_generation: Option<u64>,
    occurred_at: Instant,
}

#[derive(Debug)]
struct CrumbCache {
    crumb: Option<CachedCrumb>,
    last_failure: Option<CrumbRefreshFailure>,
}

// The mutex elects one refresher. It is intentionally held during the rare,
// timeout-bounded crumb request so other callers await its result instead of
// stampeding Yahoo with duplicate refreshes.
static YAHOO_CRUMB: Mutex<CrumbCache> = Mutex::const_new(CrumbCache {
    crumb: None,
    last_failure: None,
});

async fn get_yahoo_crumb(client: &reqwest::Client) -> Result<CachedCrumb> {
    if let Some(crumb) = YAHOO_CRUMB.lock().await.crumb.clone() {
        return Ok(crumb);
    }

    refresh_yahoo_crumb(client, None).await
}

/// Elects one asynchronous refresher. Tokio suspends waiting callers rather
/// than blocking an OS thread while the timeout-bounded request is in flight.
async fn refresh_yahoo_crumb(
    client: &reqwest::Client,
    observed_generation: Option<u64>,
) -> Result<CachedCrumb> {
    let mut cache = YAHOO_CRUMB.lock().await;
    if let Some(cached) = cache.crumb.as_ref()
        && Some(cached.generation) != observed_generation
    {
        return Ok(cached.clone());
    }

    if refresh_failure_is_active(
        cache.last_failure.as_ref(),
        observed_generation,
        Instant::now(),
    ) {
        anyhow::bail!("Yahoo Finance crumb refresh recently failed; retry shortly");
    }

    match fetch_yahoo_crumb(client).await {
        Ok(value) => {
            let crumb = CachedCrumb {
                value,
                generation: cache
                    .crumb
                    .as_ref()
                    .map_or(1, |cached| cached.generation.saturating_add(1)),
            };
            cache.crumb = Some(crumb.clone());
            cache.last_failure = None;
            Ok(crumb)
        }
        Err(error) => {
            cache.last_failure = Some(CrumbRefreshFailure {
                observed_generation,
                occurred_at: Instant::now(),
            });
            Err(error)
        }
    }
}

fn refresh_failure_is_active(
    failure: Option<&CrumbRefreshFailure>,
    observed_generation: Option<u64>,
    now: Instant,
) -> bool {
    failure.is_some_and(|failure| {
        failure.observed_generation == observed_generation
            && now.duration_since(failure.occurred_at) < CRUMB_FAILURE_COOLDOWN
    })
}

async fn fetch_yahoo_crumb(client: &reqwest::Client) -> Result<String> {
    let _ = client
        .get("https://fc.yahoo.com")
        .header("Accept", "*/*")
        .send()
        .await;

    let resp = client
        .get("https://query1.finance.yahoo.com/v1/test/getcrumb")
        .header("Accept", "*/*")
        .send()
        .await?
        .error_for_status()?;
    Ok(resp.text().await?)
}

/// Fetches search results from Yahoo Finance's search endpoint.
/// Handles basic symbol search.
pub async fn search_ticker(
    client: &reqwest::Client,
    query: &str,
) -> Result<Vec<YahooSearchResult>> {
    let query = query.trim();
    if query.is_empty() || query.len() > MAX_SEARCH_QUERY_LENGTH {
        return Ok(Vec::new());
    }
    let url = yahoo_search_url(query);

    let resp = client
        .get(url)
        .header("Accept", "*/*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await?
        .error_for_status()?;

    let data: YahooSearchResponse = resp.json().await?;
    Ok(data
        .quotes
        .into_iter()
        .filter(|result| normalize_symbol(&result.symbol).is_some())
        .take(MAX_SEARCH_RESULTS)
        .collect())
}

/// Fetches detailed metrics (P/E, Yield, etc.) from Yahoo's v7 endpoint.
/// Handles the differences between Stocks (using Dividend Yield) and ETFs (using 12-Mo Yield).
pub async fn fetch_details(client: &reqwest::Client, symbol: &str) -> Result<StockDetails> {
    let symbol = normalize_symbol(symbol).context("Ticker is invalid or too long")?;
    let resp = fetch_yahoo_quote(client, &symbol)
        .await?
        .error_for_status()?;

    let data: YahooQuoteResponse = resp.json().await?;

    if data.quote_response.result.is_empty() {
        return Err(anyhow::anyhow!("No data found"));
    }

    let q = &data.quote_response.result[0];

    // Polymorphic Field Logic:
    // Different asset classes (Stocks vs ETFs) store yield in different fields.
    // We try them in order of specificity.
    let final_yield = if let Some(y) = q.dividend_yield {
        Some(y)
    } else {
        q.trailing_yield.map(|y| y * 100.0)
    };

    // Fallback for Market Cap (ETFs use Net Assets)
    let mkt_cap = q.market_cap.or(q.net_assets).unwrap_or(0.0) as u64;

    // PERFORMANCE LOGIC:
    // 1. Try YTD (Common for ETFs, usually formatted as 5.0 for 5%)
    // 2. Try 52W Change (Common for Stocks, usually formatted as 0.05 for 5%)
    let perf = if let Some(ytd) = q.ytd_return {
        Some(ytd)
    } else {
        q.fifty_two_week_change
    };

    Ok(StockDetails {
        market_cap: mkt_cap,
        pe_ratio: q.pe_ratio,
        dividend_yield: final_yield,
        high_52w: q.high_52w.unwrap_or(0.0),
        low_52w: q.low_52w.unwrap_or(0.0),
        year_return: perf,
    })
}
/// Fetches a current quote from Yahoo Finance without an API key.
pub async fn fetch_quote(client: &reqwest::Client, symbol: &str) -> Result<MarketQuote> {
    let symbol = normalize_symbol(symbol).context("Ticker is invalid or too long")?;
    let chart = fetch_yahoo_chart(
        client,
        &symbol,
        &[
            ("interval", "1m"),
            ("range", "1d"),
            ("events", "div|split|capitalGains"),
        ],
    )
    .await?;
    let metadata = chart.meta;

    let price = metadata
        .regular_market_price
        .ok_or_else(|| anyhow::anyhow!("Yahoo Finance returned no current price for {symbol}"))?;
    let previous_close = metadata
        .previous_close
        .or(metadata.chart_previous_close)
        .ok_or_else(|| anyhow::anyhow!("Yahoo Finance returned no previous close for {symbol}"))?;

    market_quote_from_prices(price, previous_close)
}
/// Fetches historical stock data from Yahoo Finance API.
/// The data points are returned as a vector of (timestamp, close price) tuples.
/// Used by the charting component.
pub async fn fetch_history(client: &reqwest::Client, symbol: &str) -> Result<Vec<(f64, f64)>> {
    let symbol = normalize_symbol(symbol).context("Ticker is invalid or too long")?;
    let end = OffsetDateTime::now_utc();
    let start = end - time::Duration::days(365);
    let start_timestamp = start.unix_timestamp().to_string();
    let end_timestamp = end.unix_timestamp().to_string();
    let chart = fetch_yahoo_chart(
        client,
        &symbol,
        &[
            ("period1", start_timestamp.as_str()),
            ("period2", end_timestamp.as_str()),
            ("interval", "1d"),
            ("events", "div|split|capitalGains"),
        ],
    )
    .await?;
    history_points_from_chart(chart)
}
/// Fetches the Sidebar-visible quotes and outputs Waybar-compatible JSON.
/// The compatibility format lets existing external status-bar integrations
/// continue working while Genoa displays it in Sidebar.
pub async fn run_widget_mode(config: &Config, client: &reqwest::Client) -> Result<()> {
    let results = stream::iter(
        config
            .stocks
            .iter()
            .filter(|stock| stock.sidebar)
            .filter_map(|stock| normalize_symbol(&stock.symbol))
            .take(MAX_SIDEBAR_QUOTES)
            .map(|symbol| {
                let client = client.clone();
                async move {
                    let quote = fetch_quote(&client, &symbol).await;
                    (symbol, quote)
                }
            }),
    )
    .buffered(MAX_CONCURRENT_QUOTES)
    .collect::<Vec<_>>()
    .await;
    let mut text_parts = Vec::new();
    let mut tooltip_parts = Vec::new();
    for (symbol, result) in results {
        match result {
            Ok(quote) => {
                let (color, icon) = if quote.percent >= 0.0 {
                    ("#a6e3a1", "")
                } else {
                    ("#f38ba8", "")
                };
                let escaped_symbol = escape_pango(&symbol);
                let part = format!(
                    "<span color='{}'>{} {:.2} {}</span>",
                    color, escaped_symbol, quote.price, icon
                );
                text_parts.push(part);
                tooltip_parts.push(format!(
                    "<span color='{}'>{}: ${:.2} ({:.2}%)</span>",
                    color, escaped_symbol, quote.price, quote.percent
                ));
            }
            Err(_) => {
                text_parts.push(format!(
                    "<span color='#6c7086'>{} ???</span>",
                    escape_pango(&symbol)
                ));
            }
        }
    }
    let output = WaybarOutput {
        text: text_parts.join(" "),
        tooltip: tooltip_parts.join("\n"),
        class: "finance".to_string(),
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

fn yahoo_search_url(query: &str) -> reqwest::Url {
    let mut url = reqwest::Url::parse("https://query2.finance.yahoo.com/v1/finance/search")
        .expect("Yahoo Finance search base URL is valid");
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("lang", "en-US");
    url
}

fn yahoo_quote_url(symbols: &str, crumb: &str) -> reqwest::Url {
    let mut url = reqwest::Url::parse("https://query1.finance.yahoo.com/v7/finance/quote")
        .expect("Yahoo Finance quote base URL is valid");
    url.query_pairs_mut()
        .append_pair("symbols", symbols)
        .append_pair("crumb", crumb);
    url
}

fn escape_pango(value: &str) -> Cow<'_, str> {
    if !value
        .bytes()
        .any(|byte| matches!(byte, b'&' | b'<' | b'>' | b'\'' | b'"'))
    {
        return Cow::Borrowed(value);
    }

    let mut escaped = String::with_capacity(value.len().saturating_mul(6));
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\'' => escaped.push_str("&apos;"),
            '"' => escaped.push_str("&quot;"),
            _ => escaped.push(character),
        }
    }
    Cow::Owned(escaped)
}

async fn fetch_yahoo_quote(client: &reqwest::Client, symbols: &str) -> Result<reqwest::Response> {
    let crumb = get_yahoo_crumb(client).await?;
    let response = client
        .get(yahoo_quote_url(symbols, &crumb.value))
        .send()
        .await?;

    if !is_crumb_rejection(response.status()) {
        return Ok(response);
    }

    drop(response);
    let refreshed = refresh_yahoo_crumb(client, Some(crumb.generation)).await?;
    Ok(client
        .get(yahoo_quote_url(symbols, &refreshed.value))
        .send()
        .await?)
}

fn is_crumb_rejection(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    )
}

async fn fetch_yahoo_chart(
    client: &reqwest::Client,
    symbol: &str,
    query: &[(&str, &str)],
) -> Result<YahooChartResult> {
    let url = yahoo_chart_url(symbol, query)?;

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Yahoo Finance could not load data for {symbol}"))?
        .error_for_status()
        .with_context(|| format!("Yahoo Finance rejected the request for {symbol}"))?;
    let response: YahooChartResponse = response
        .json()
        .await
        .with_context(|| format!("Yahoo Finance returned invalid data for {symbol}"))?;

    response
        .chart
        .result
        .and_then(|mut results| results.pop())
        .ok_or_else(|| {
            let detail = response
                .chart
                .error
                .and_then(|error| error.description)
                .unwrap_or_else(|| "no chart data returned".to_string());
            anyhow::anyhow!("Yahoo Finance could not load {symbol}: {detail}")
        })
}

fn yahoo_chart_url(symbol: &str, query: &[(&str, &str)]) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse("https://query1.finance.yahoo.com/v8/finance/chart/")
        .expect("Yahoo Finance chart base URL is valid");
    url.path_segments_mut()
        .map_err(|()| anyhow::anyhow!("Yahoo Finance chart URL cannot accept a ticker path"))?
        .pop_if_empty()
        .push(symbol);
    {
        let mut query_pairs = url.query_pairs_mut();
        query_pairs.append_pair("symbol", symbol);
        for (key, value) in query {
            query_pairs.append_pair(key, value);
        }
    }
    Ok(url)
}

fn market_quote_from_prices(price: f64, previous_close: f64) -> Result<MarketQuote> {
    if !price.is_finite() || !previous_close.is_finite() || previous_close == 0.0 {
        anyhow::bail!("Yahoo Finance returned invalid price data");
    }

    Ok(MarketQuote {
        price,
        percent: ((price - previous_close) / previous_close) * 100.0,
    })
}

fn history_points_from_chart(chart: YahooChartResult) -> Result<Vec<(f64, f64)>> {
    let timestamps = chart
        .timestamp
        .context("Yahoo Finance returned no timestamps")?;
    let closes = chart
        .indicators
        .quote
        .first()
        .context("Yahoo Finance returned no quote data")?
        .close
        .iter();
    let points: Vec<(f64, f64)> = timestamps
        .into_iter()
        .zip(closes)
        .filter_map(|(timestamp, close)| close.map(|close| (timestamp as f64, close)))
        .collect();
    if points.is_empty() {
        return Err(anyhow::anyhow!("History data is empty"));
    }
    Ok(points)
}

/// Fetches market status including yields for 10Y, 5Y, and 3M Treasuries from Yahoo Finance.
/// Used for displaying yield data and yield curve in app's top banner.
pub async fn fetch_market_status(client: &reqwest::Client) -> Result<MarketStatus> {
    let resp = fetch_yahoo_quote(client, "^TNX,^FVX,^IRX")
        .await?
        .error_for_status()?;

    let data: YahooQuoteResponse = resp.json().await?;
    let results = data.quote_response.result;

    // 3. Map results
    // We need to find which is which because lists aren't always ordered
    let mut y10 = 0.0;
    let mut y5 = 0.0;
    let mut y3m = 0.0;

    for q in results {
        let val = q.regular_market_price.unwrap_or(0.0); // We need to add regularMarketPrice to YahooQuote struct!
        match q.symbol.as_str() {
            "^TNX" => y10 = val,
            "^FVX" => y5 = val,
            "^IRX" => y3m = val,
            _ => {}
        }
    }

    Ok(MarketStatus {
        yield_10y: y10,
        yield_5y: y5,
        yield_3m: y3m,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CRUMB_FAILURE_COOLDOWN, CrumbRefreshFailure, YahooChartResponse, escape_pango,
        history_points_from_chart, is_crumb_rejection, market_quote_from_prices,
        refresh_failure_is_active, yahoo_chart_url, yahoo_quote_url, yahoo_search_url,
    };
    use std::borrow::Cow;
    use std::time::Instant;

    #[test]
    fn current_quote_uses_the_previous_close_for_daily_change() {
        let quote = market_quote_from_prices(110.0, 100.0).unwrap();

        assert_eq!(quote.price, 110.0);
        assert_eq!(quote.percent, 10.0);
    }

    #[test]
    fn current_quote_rejects_an_invalid_previous_close() {
        assert!(market_quote_from_prices(110.0, 0.0).is_err());
    }

    #[test]
    fn chart_history_ignores_missing_close_values() {
        let response: YahooChartResponse = serde_json::from_str(
            r#"{
                "chart": {
                    "result": [{
                        "meta": {},
                        "timestamp": [100, 200, 300],
                        "indicators": {"quote": [{"close": [10.0, null, 12.5]}]}
                    }],
                    "error": null
                }
            }"#,
        )
        .unwrap();
        let chart = response.chart.result.unwrap().into_iter().next().unwrap();

        assert_eq!(
            history_points_from_chart(chart).unwrap(),
            vec![(100.0, 10.0), (300.0, 12.5)]
        );
    }

    #[test]
    fn chart_url_has_one_separator_before_the_ticker() {
        let url = yahoo_chart_url("SPY", &[("range", "1d")]).unwrap();

        assert_eq!(url.path(), "/v8/finance/chart/SPY");
        assert_eq!(url.query(), Some("symbol=SPY&range=1d"));
    }

    #[test]
    fn query_builders_percent_encode_untrusted_values() {
        let search_url = yahoo_search_url("SPY&lang=attacker");
        let quote_url = yahoo_quote_url("SPY&symbols=attacker", "crumb&override=true");

        assert!(search_url.as_str().contains("SPY%26lang%3Dattacker"));
        assert!(quote_url.as_str().contains("SPY%26symbols%3Dattacker"));
        assert!(quote_url.as_str().contains("crumb%26override%3Dtrue"));
    }

    #[test]
    fn pango_escaping_neutralizes_markup_characters() {
        assert_eq!(
            escape_pango("<ticker&'\">"),
            "&lt;ticker&amp;&apos;&quot;&gt;"
        );
        assert!(matches!(escape_pango("SPY"), Cow::Borrowed("SPY")));
    }

    #[test]
    fn only_auth_responses_trigger_a_crumb_refresh() {
        assert!(is_crumb_rejection(reqwest::StatusCode::UNAUTHORIZED));
        assert!(is_crumb_rejection(reqwest::StatusCode::FORBIDDEN));
        assert!(!is_crumb_rejection(reqwest::StatusCode::TOO_MANY_REQUESTS));
    }

    #[test]
    fn crumb_refresh_failures_are_brief_and_generation_scoped() {
        let now = Instant::now();
        let failure = CrumbRefreshFailure {
            observed_generation: Some(7),
            occurred_at: now,
        };

        assert!(refresh_failure_is_active(Some(&failure), Some(7), now));
        assert!(!refresh_failure_is_active(Some(&failure), Some(8), now));
        assert!(!refresh_failure_is_active(
            Some(&failure),
            Some(7),
            now + CRUMB_FAILURE_COOLDOWN,
        ));
    }
}
