fn main() {
    #[cfg(windows)]
    {
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("assets/agsh.ico");
        resource.compile().expect("failed to compile resources");
    }
}
