// Empêche l'ouverture d'une console supplémentaire sous Windows en release. NE PAS RETIRER.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `minestrator-terminal --mcp` : sert le protocole MCP (stdio) au lieu de la GUI.
    if std::env::args().any(|a| a == "--mcp") {
        minestrator_terminal_lib::run_mcp();
    } else {
        minestrator_terminal_lib::run();
    }
}
