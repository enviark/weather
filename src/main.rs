use serde::{Deserialize, Serialize};

use chrono::{Date, Datelike, Local};
use tinytemplate::TinyTemplate;
use weather_helpers;
use weather_helpers::Season;

use fastly::geo::{geo_lookup, Geo};
use fastly::Dictionary;
use fastly::{
    http::{header, Method, StatusCode},
    Error, Request, Response,
};

// Define a constant for the backend name, as shown in your Fastly service:
const BACKEND_NAME: &str = "api.openweathermap.org";

#[derive(Deserialize)]
struct QueryParams {
    units: Option<String>,
}

/// The entry point for your application.
#[fastly::main]
fn main(req: Request) -> Result<Response, Error> {
    // Return early if the request method is not GET.
    if req.get_method() != Method::GET {
        return Ok(Response::from_status(StatusCode::METHOD_NOT_ALLOWED)
            .with_body("This method is not allowed"));
    }

    let resp = match req.get_path() {
        "/" => {
            // Get the end user's location
            let location = geo_lookup(req.get_client_ip_addr().unwrap()).unwrap();
            // Get the local time
            let local = Local::now().date();

            // Log output helps you debug issues when developing your service.
            // Run `fastly log-tail` to see this output live as you make requests.
            println!(
                "Requesting weather for {}, {} ({}, {})",
                location.latitude(),
                location.longitude(),
                location.city(),
                location.country_name()
            );

            // Fetch the query string and parse it into the `QueryParams` type
            let query: QueryParams = req.get_query()?;

            // Get units from query params, or default to "metric"
            let units = match query.units {
                Some(units) => units,
                None => String::from("metric"),
            };

            // Build the API request, and set the cache override to PASS
            let url = format!(
                "http://api.openweathermap.org/data/2.5/onecall?lat={}&lon={}&appid={}&units={}",
                location.latitude(),
                location.longitude(),
                get_api_key(),
                units
            );
            let bereq = Request::new(Method::GET, url)
                .with_header(header::HOST, "api.openweathermap.org")
                .with_pass(true);

            // Send the request to the backend
            let mut beresp = bereq.send(BACKEND_NAME)?;

            // Get the response body into an APIResponse
            let api_response = beresp.take_body_json::<APIResponse>()?;

            let body_response = generate_view(api_response, location, local, &units);

            Response::from_body(body_response)
                .with_status(StatusCode::OK)
                .with_content_type(fastly::mime::TEXT_HTML_UTF_8)
        }
        // Serve dynamic background image based on season
        "/bg-image.jpg" => {
            let location = geo_lookup(req.get_client_ip_addr().unwrap()).unwrap();
            let local = Local::now().date();
            let image: &[u8] = match weather_helpers::get_season(location, local) {
                Season::Summer => include_bytes!("static/img/summer.jpg"),
                Season::Autumn => include_bytes!("static/img/autumn.jpg"),
                Season::Winter => include_bytes!("static/img/winter.jpg"),
                Season::Spring => include_bytes!("static/img/spring.jpg"),
            };

            Response::from_body(image)
                .with_status(StatusCode::OK)
                .with_content_type(fastly::mime::IMAGE_JPEG)
        }

        // Serve static CSS and JS files
        "/style.css" => Response::from_body(include_str!("static/style.css"))
            .with_content_type(fastly::mime::TEXT_CSS),
        "/feather.min.js" => Response::from_body(include_str!("static/feather.min.js"))
            .with_content_type(fastly::mime::TEXT_JAVASCRIPT),

        // Catch all other requests and return a 404.
        _ => Response::from_body("The page you requested could not be found")
            .with_status(StatusCode::NOT_FOUND),
    };

    Ok(resp)
}

/// Context for TinyTemplate
#[derive(Serialize)]
struct TemplateContext {
    day: String,
    day_short: String,
    date: String,
    city: String,
    temp: String,
    rain: String,
    wind: String,
    humidity: String,
    description: String,
    icon: String,
    next_days: Vec<NextDay>,
    is_metric: bool,
}

/// Struct representing API response
#[derive(Deserialize)]
struct APIResponse {
    current: CurrentReport,
    daily: Vec<DailyReport>,
    minutely: Vec<MinutelyReport>,
}

/// Struct representing a single response entry
#[derive(Deserialize)]
struct CurrentReport {
    temp: f32,
    wind_speed: f32,
    humidity: f32,
    weather: Vec<WeatherReport>,
}

/// Struct representing a single day's weather
#[derive(Deserialize)]
struct DailyReport {
    dt: i32,
    temp: Temperatures,
    weather: Vec<WeatherReport>,
}

/// Struct representing a single weather report
#[derive(Deserialize)]
struct WeatherReport {
    description: String,
    icon: String,
}

/// Struct representing precipitation data
#[derive(Deserialize)]
struct MinutelyReport {
    precipitation: f32,
}

/// Struct representing a set of temperatures
#[derive(Deserialize)]
struct Temperatures {
    day: f32,
}

/// Basic struct with minimal info about the next days
#[derive(Serialize)]
struct NextDay {
    day: String,
    temp: String,
    icon: String,
}

fn generate_view(
    api_response: APIResponse,
    location: Geo,
    local: Date<Local>,
    units: &str,
) -> String {
    // Initialize template
    let mut tt = TinyTemplate::new();
    tt.add_template("weather", include_str!("static/index.html"))
        .unwrap();

    // Get the data for the next three days and put them in a vector to iterate them later in
    // the template
    let mut next_days: Vec<NextDay> = Vec::new();
    for i in 0..3 {
        next_days.push(NextDay {
            day: weather_helpers::datetime_to_day(format!("{}", api_response.daily[i + 1].dt)),
            temp: (api_response.daily[i + 1].temp.day as i32).to_string(),
            icon: weather_helpers::get_feather_weather_icon(
                &api_response.daily[i + 1].weather[0].icon,
            ),
        });
    }

    // Fill the template context
    let context = TemplateContext {
        day: weather_helpers::weekday_full(local.weekday().to_string()),
        day_short: local.weekday().to_string(),
        date: local.format("%e %B %Y").to_string(),
        city: String::from(location.city()),
        temp: (api_response.current.temp as i32).to_string(),
        rain: format!("{}", api_response.minutely[0].precipitation),
        wind: format!("{}", api_response.current.wind_speed),
        humidity: format!("{}", api_response.current.humidity),
        description: format!("{}", api_response.current.weather[0].description).replace("\"", ""),
        icon: weather_helpers::get_feather_weather_icon(&api_response.current.weather[0].icon),
        next_days,
        is_metric: units == "metric",
    };

    tt.render("weather", &context).unwrap()
}

fn get_api_key() -> String {
    match Dictionary::open("weather_auth").get("key") {
        Some(key) => key,
        None => panic!("No OpenWeatherMap API key!"),
    }
}
