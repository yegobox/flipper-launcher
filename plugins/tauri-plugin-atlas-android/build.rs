// Atlas calls this plugin from Rust only, so it exposes no JS commands.
const COMMANDS: &[&str] = &[];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).android_path("android").build();
}
