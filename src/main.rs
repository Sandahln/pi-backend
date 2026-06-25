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

// === FILE HELPERS ===

/// Read a JSON file containing a username -> value map.
fn read_user_map(path: &str) -> HashMap<String, serde_json::Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<HashMap<String, serde_json::Value>>(&s).ok())
        .unwrap_or_default()
}

/// Write a username -> value map as pretty JSON to a file.
fn write_user_map(path: &str, map: &HashMap<String, serde_json::Value>) -> bool {
    serde_json::to_string_pretty(map)
        .ok()
        .map(|s| fs::write(path, s).is_ok())
        .unwrap_or(false)
}

/// Extract the knockout value from an entry regardless of where it is nested.
fn extract_knockout_value(entry: &serde_json::Value) -> Option<serde_json::Value> {
    // Top-level knockout (new format and legacy db.json)
    if let Some(ko) = entry.get("knockout") {
        if !ko.is_null() {
            return Some(ko.clone());
        }
    }
    // Nested inside predictions (old data.txt format)
    if let Some(preds) = entry.get("predictions") {
        if let Some(ko) = preds.get("knockout") {
            if !ko.is_null() {
                return Some(ko.clone());
            }
        }
    }
    None
}

/// Add or replace the top-level "knockout" field on an entry object.
fn merge_knockout_into(entry: &mut serde_json::Value, ko: serde_json::Value) {
    if let Some(obj) = entry.as_object_mut() {
        obj.insert("knockout".to_string(), ko);
    }
}

// === NEW SAVE ENDPOINTS ===

/// POST /api/save/groupstage — saves group stage predictions to groupstage.json
#[post("/api/save/groupstage")]
async fn save_groupstage(body: web::Json<serde_json::Value>) -> impl Responder {
    let user = match body.get("user").and_then(|u| u.as_str()) {
        Some(u) => u.to_string(),
        None => return HttpResponse::BadRequest().body("Missing 'user' field"),
    };
    let mut map = read_user_map("groupstage.json");
    map.insert(user.clone(), body.into_inner());
    if write_user_map("groupstage.json", &map) {
        println!("Saved group stage for: {}", user);
        HttpResponse::Ok().body("Group stage saved!")
    } else {
        HttpResponse::InternalServerError().body("Failed to write groupstage.json")
    }
}

/// POST /api/save/knockout — saves knockout predictions to knockout.json
#[post("/api/save/knockout")]
async fn save_knockout(body: web::Json<serde_json::Value>) -> impl Responder {
    let user = match body.get("user").and_then(|u| u.as_str()) {
        Some(u) => u.to_string(),
        None => return HttpResponse::BadRequest().body("Missing 'user' field"),
    };
    let mut map = read_user_map("knockout.json");
    map.insert(user.clone(), body.into_inner());
    if write_user_map("knockout.json", &map) {
        println!("Saved knockout for: {}", user);
        HttpResponse::Ok().body("Knockout saved!")
    } else {
        HttpResponse::InternalServerError().body("Failed to write knockout.json")
    }
}

// === LEGACY SAVE (kept for backward compatibility) ===
#[post("/api/save")]
async fn save_data(data: web::Json<WorldCupData>) -> impl Responder {
    // Route to the appropriate new file based on payload content
    let is_knockout_only = data.predictions.get("knockout").is_some()
        && data.predictions.get("predictions").is_none();

    if is_knockout_only {
        let body = serde_json::json!({
            "user": data.user,
            "game_id": data.game_id,
            "knockout": data.predictions.get("knockout").cloned().unwrap_or(serde_json::json!(null)),
            "updatedAt": data.predictions.get("updatedAt").cloned().unwrap_or(serde_json::json!(null))
        });
        let mut map = read_user_map("knockout.json");
        map.insert(data.user.clone(), body);
        if write_user_map("knockout.json", &map) {
            println!("Saved knockout (via /api/save) for: {}", data.user);
            return HttpResponse::Ok().body("Data successfully saved to disk!");
        }
    } else {
        let body = serde_json::json!({
            "user": data.user,
            "game_id": data.game_id,
            "predictions": data.predictions
        });
        let mut map = read_user_map("groupstage.json");
        map.insert(data.user.clone(), body);
        if write_user_map("groupstage.json", &map) {
            println!("Saved group stage (via /api/save) for: {}", data.user);
            // Also keep data.txt append for legacy backup
            let json_string = match serde_json::to_string(&*data) {
                Ok(s) => s,
                Err(_) => return HttpResponse::InternalServerError().body("Error parsing JSON"),
            };
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open("data.txt") {
                let _ = writeln!(file, "{}", json_string);
            }
            return HttpResponse::Ok().body("Data successfully saved to disk!");
        }
    }

    HttpResponse::InternalServerError().body("Failed to save data")
}

// === LOAD OPERATION — merges all sources ===
#[get("/api/load")]
async fn get_data() -> impl Responder {
    let mut merged: HashMap<String, serde_json::Value> = HashMap::new();

    // 1. Legacy: db.json (userBettings array — lowest priority)
    if let Ok(contents) = fs::read_to_string("db.json") {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) {
            if let Some(bettings) = json.get("userBettings").and_then(|v| v.as_array()) {
                for entry in bettings {
                    if let Some(name) = entry.get("user").or(entry.get("name")).and_then(|v| v.as_str()) {
                        merged.insert(name.to_string(), entry.clone());
                    }
                }
            }
        }
    }

    // 2. Legacy: data.txt — collect group stage and knockout entries *separately*
    //    to avoid the old problem of one overwriting the other.
    let mut txt_group: HashMap<String, serde_json::Value> = HashMap::new();
    let mut txt_knockout_map: HashMap<String, serde_json::Value> = HashMap::new();
    if let Ok(contents) = fs::read_to_string("data.txt") {
        for line in contents.lines().filter(|l| !l.trim().is_empty()) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(user) = v.get("user").or(v.get("name")).and_then(|u| u.as_str()).map(|s| s.to_string()) {
                    let is_ko_only = v.get("predictions")
                        .map(|p| p.get("knockout").is_some() && p.get("predictions").is_none())
                        .unwrap_or(false);
                    if is_ko_only {
                        txt_knockout_map.insert(user, v);
                    } else {
                        txt_group.insert(user, v);
                    }
                }
            }
        }
    }
    for (user, gs) in txt_group {
        merged.insert(user, gs);
    }
    for (user, ko_entry) in txt_knockout_map {
        let ko_val = ko_entry.get("predictions").and_then(|p| p.get("knockout")).cloned();
        if let Some(ko) = ko_val {
            if let Some(entry) = merged.get_mut(&user) {
                merge_knockout_into(entry, ko);
            } else {
                merged.insert(user, ko_entry);
            }
        }
    }

    // 3. New: groupstage.json — highest priority for group stage data
    for (user, gs) in read_user_map("groupstage.json") {
        // Preserve existing knockout from merged entry before replacing
        let existing_ko = merged.get(&user).and_then(|e| extract_knockout_value(e));
        merged.insert(user.clone(), gs);
        if let Some(ko) = existing_ko {
            // Only restore if the groupstage.json entry itself has no knockout
            if extract_knockout_value(merged.get(&user).unwrap()).is_none() {
                if let Some(entry) = merged.get_mut(&user) {
                    merge_knockout_into(entry, ko);
                }
            }
        }
    }

    // 4. New: knockout.json — highest priority for knockout data
    for (user, ko_entry) in read_user_map("knockout.json") {
        if let Some(ko) = ko_entry.get("knockout").cloned() {
            if let Some(entry) = merged.get_mut(&user) {
                merge_knockout_into(entry, ko);
            } else {
                merged.insert(user, ko_entry);
            }
        }
    }

    let mut all_entries: Vec<serde_json::Value> = merged.into_values().collect();
    all_entries.sort_by_key(|a| a.get("user").or(a.get("name")).and_then(|v| v.as_str()).unwrap_or("").to_lowercase());

    HttpResponse::Ok().json(all_entries)
}

#[get("/api/pages")]
async fn get_pages() -> impl Responder {
    let contents = fs::read_to_string("db.json").unwrap_or_else(|_| "{}".to_string());
    let json: serde_json::Value = serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}));
    let pages = json.get("pages").cloned().unwrap_or_else(|| serde_json::json!([]));
    HttpResponse::Ok().json(pages)
}

#[get("/api/games")]
async fn get_games() -> impl Responder {
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
    let host = std::env::var("API_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port_str = std::env::var("API_PORT").unwrap_or_else(|_| "8080".to_string());
    let port = port_str.parse::<u16>().unwrap_or(8080);

    println!("Rust API actively listening on http://{}:{}", host, port);

    HttpServer::new(|| {
        let cors = Cors::permissive();

        App::new()
            .wrap(cors)
            .service(save_groupstage)
            .service(save_knockout)
            .service(save_data)
            .service(get_data)
            .service(get_pages)
            .service(get_games)
    })
    .bind((host, port))?
    .run()
    .await
}