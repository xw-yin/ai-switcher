fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon_with_id("assets/AppIcon.ico", "1");
        res.set("ProductName", "AI Switcher");
        res.set("FileDescription", "AI Switcher - Codex & Antigravity Account Manager");
        res.compile().expect("embed Windows icon resource");
    }
}
