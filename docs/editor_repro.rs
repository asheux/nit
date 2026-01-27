// Editor repro: tabs, unicode, multiline strings/comments, and long lines.

fn main() {
	let ascii = "plain";
	let unicode = "café naïve — 東京 🚀";
	let long_line = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.";
	let multiline = "line 1\nline 2\nline 3";
	let raw = r#"first line
second line
third line"#;
	// tabbed columns: one	two	three
	let columns = "one	 two	 three";

	/*
	multi-line comment with unicode:
	- déjà vu
	- jalapeño
	- 🚧
	*/
	println!("{ascii} {unicode} {long_line} {multiline} {raw} {columns}");
}
