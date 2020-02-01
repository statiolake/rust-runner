use lazy_static::lazy_stati;
use proconio::input;

lazy_static! {
    static ref HELLO: String = "hello".to_string();
}

fn main() {
    input!(name: String);
    println!("{}, {}", *HELLO, name);
}
