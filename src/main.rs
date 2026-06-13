use actix_cors::Cors;
use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;

#[derive(Debug, Serialize, Deserialize)]
struct WorldCupData {
    user: String,
    game_id: String,
    predictions: serde_json::Value,
}

// === 1. THE SAVE OPERATION ===
#[post("/api/save")]
async fn save_data(data: web::Json<WorldCupData>) -> impl Responder {
    let json_string = match serde_json::to_string(&*data) {
        Ok(s) => s,
        Err(_) => return HttpResponse::InternalServerError().body("Error parsing JSON"),
    };

    let mut file = match OpenOptions::new()
        .create(true)
        .append(true)
        .open("data.txt")
    {
        Ok(f) => f,
        Err(_) => return HttpResponse::InternalServerError().body("Could not open file"),
    };

    if writeln!(file, "{}", json_string).is_ok() {
        println!("Successfully saved data for: {}", data.user);
        HttpResponse::Ok().body("Data successfully saved to disk!")
    } else {
        HttpResponse::InternalServerError().body("Failed to write data")
    }
}

// === 2. THE READ OPERATION ===
#[get("/api/load")]
async fn get_data() -> impl Responder {
    let mut user_map: HashMap<String, serde_json::Value> = HashMap::new();

    // 1. Try to load legacy users from db.json (the "userBettings" key)
    if let Ok(contents) = fs::read_to_string("db.json") {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) {
            if let Some(bettings) = json.get("userBettings").and_then(|v| v.as_array()) {
                for entry in bettings {
                    if let Some(name) = entry.get("user").or(entry.get("name")).and_then(|v| v.as_str()) {
                        user_map.insert(name.to_string(), entry.clone());
                    }
                }
            }
        }
    }

    // 2. Try to load new users from data.txt (appended lines)
    if let Ok(contents) = fs::read_to_string("data.txt") {
        for line in contents.lines() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(user) = v.get("user").or(v.get("name")).and_then(|u| u.as_str()) {
                    user_map.insert(user.to_string(), v);
                }
            }
        }
    }

    let mut all_entries: Vec<serde_json::Value> = user_map.into_values().collect();
    all_entries.sort_by_key(|a| a.get("user").or(a.get("name")).and_then(|v| v.as_str()).unwrap_or("").to_lowercase());

    HttpResponse::Ok().json(all_entries)
}

#[get("/api/pages")]
async fn get_pages() -> impl Responder {
    let contents = fs::read_to_string("db.json").unwrap_or_else(|_| "{}".to_string());
    let json: serde_json::Value = serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}));
    
    // Return the "pages" array, or an empty array [] if not found
    let pages = json.get("pages").cloned().unwrap_or_else(|| serde_json::json!([]));
    HttpResponse::Ok().json(pages)
}

#[get("/api/games")]
async fn get_games() -> impl Responder {
    // Using a more resilient approach to prevent 500 errors
    match fs::read_to_string("db.json") {
        Ok(contents) => {
            let json: serde_json::Value = match serde_json::from_str(&contents) {
                Ok(v) => v,
                Err(_) => return HttpResponse::Ok().json(Vec::<String>::new()),
            };
            HttpResponse::Ok().json(json.get("games").unwrap_or(&serde_json::json!([])))
        }
        Err(_) => HttpResponse::Ok().json(Vec::<String>::new()),
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // 1. Check if a custom host was passed via the environment, default to localhost
    let host = std::env::var("API_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    
    // 2. Check for a custom port, default to 8080
    let port_str = std::env::var("API_PORT").unwrap_or_else(|_| "8080".to_string());
    let port = port_str.parse::<u16>().unwrap_or(8080);

    println!("Rust API actively listening on http://{}:{}", host, port);

    HttpServer::new(|| {
        let cors = Cors::permissive(); 

        App::new()
            .wrap(cors)
            .service(save_data)
            .service(get_data)
            .service(get_pages)
            .service(get_games)
    })
    .bind((host, port))? // <-- Dynamic binding!
    .run()
    .await
}