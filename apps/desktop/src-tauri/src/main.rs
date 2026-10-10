mod notifications;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tauri::{
    Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_notification::NotificationExt;

const SERVER: &str = "http://127.0.0.1:4318";
fn show(app: &tauri::AppHandle, messages: bool) {
    if let Some(window) = app.get_webview_window("main") {
        if messages {
            let _ = window.navigate(
                format!("{SERVER}/console/#view=notifications")
                    .parse()
                    .unwrap(),
            );
        }
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
fn external(url: &tauri::Url) {
    // Never pass arbitrary protocols to the OS. No shell interpretation, no IPC.
    if url.scheme() == "https" && url.username().is_empty() && url.password().is_none() {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("/usr/bin/open")
                .arg(url.as_str())
                .spawn();
        }
    }
}
fn local(url: &tauri::Url) -> bool {
    url.scheme() == "tauri"
        || (url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.port() == Some(4318)
            && url.path().starts_with("/console/"))
}
fn main() {
    let app=tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app,_,_|show(app,false)))
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let preference=app.path().app_data_dir()?.join("notifications.json");
            let enabled=Arc::new(AtomicBool::new(std::fs::read_to_string(&preference).ok().and_then(|s|serde_json::from_str::<bool>(&s).ok()).unwrap_or(false)));
            WebviewWindowBuilder::new(app,"main",WebviewUrl::App("index.html".into()))
                .title("Feynman").inner_size(1440.,980.).min_inner_size(900.,640.)
                .on_navigation(|url|{if local(url){true}else{external(url);false}})
                .on_new_window(|url,_|{external(&url);tauri::webview::NewWindowResponse::Deny})
                .build()?;
            let open=MenuItem::with_id(app,"open","打开工作台",true,Some("CmdOrCtrl+O"))?;
            let messages=MenuItem::with_id(app,"messages","消息中心",true,None::<&str>)?;
            let toggle=CheckMenuItem::with_id(app,"notifications","启用系统通知",true,enabled.load(Ordering::Relaxed),None::<&str>)?;
            let test=MenuItem::with_id(app,"test","发送测试通知",true,None::<&str>)?;
            let settings=MenuItem::with_id(app,"settings","macOS 通知设置…",true,None::<&str>)?;
            let quit=MenuItem::with_id(app,"quit","退出客户端（Server 继续运行）",true,Some("CmdOrCtrl+Q"))?;
            let menu=Menu::with_items(app,&[&open,&messages,&toggle,&test,&settings,&quit])?;
            let preference_enabled=enabled.clone();
            TrayIconBuilder::new().icon(app.default_window_icon().unwrap().clone()).tooltip("Feynman · 本地项目管家")
                .menu(&menu).on_menu_event(move |app,event|match event.id.as_ref() {
                    "open"=>show(app,false),"messages"=>show(app,true),"quit"=>app.exit(0),
                    "notifications"=>{
                        let value=toggle.is_checked().unwrap_or(false);
                        preference_enabled.store(value,Ordering::Relaxed);
                        if let Some(parent)=preference.parent(){let _=std::fs::create_dir_all(parent);}
                        if let Err(e)=std::fs::write(&preference,if value{"true"}else{"false"}){eprintln!("notification preference: {e}");}
                    },
                    "test"=>{
                        if preference_enabled.load(Ordering::Relaxed){let _=app.notification().builder().title("Feynman · 测试通知").body("通知链路已触发。真实显示由 macOS 通知设置与专注模式决定。").show();}
                    },
                    "settings"=>{#[cfg(target_os="macos")] {let _=std::process::Command::new("/usr/bin/open").arg("x-apple.systempreferences:com.apple.Notifications-Settings.extension").spawn();}},
                    _=>{}
                }).build(app)?;
            let handle=app.handle().clone();
            tauri::async_runtime::spawn(notifications::listen(handle,enabled));
            Ok(())
        })
        .on_window_event(|window,event| {
            if let WindowEvent::CloseRequested {api,..}=event {api.prevent_close();let _=window.hide();}
        })
        .build(tauri::generate_context!()).expect("cannot initialize Feynman desktop");
    app.run(|app, event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            show(app, false);
        }
        let _ = (app, event);
    });
}
