use inspirum_terminal::app::App;
#[test]
fn connection_form_renders_and_corrupt_storage_cannot_be_overwritten() {
 let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("profiles.json");
 std::fs::write(&path,b"broken").unwrap();
 let mut app=App::new(path.clone(),None);
 assert!(!app.storage_writable());
 let ctx=eframe::egui::Context::default();
 let output=ctx.run(eframe::egui::RawInput::default(),|ctx| app.ui(ctx));
 assert!(!output.shapes.is_empty());
 assert_eq!(std::fs::read(path).unwrap(),b"broken");
}
