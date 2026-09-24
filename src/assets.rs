use axum::response::Html;
pub fn index() -> Html<&'static str> { Html("<html><body>placeholder</body></html>") }
