// Empêche l'ouverture d'une console supplémentaire sous Windows en debug desktop. NE PAS RETIRER.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Point d'entrée pour un run desktop de test (`cargo run`). Sur Android/iOS, l'entrée
// est `run()` via `#[tauri::mobile_entry_point]` (appelée par l'activité native).
fn main() {
    minestrator_terminal_mobile_lib::run();
}
