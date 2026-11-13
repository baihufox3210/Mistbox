/// 中文文件註解
pub fn fib(n: u64) -> u64 {
    match n {
        0 | 1 => n,
        _ => fib(n - 1) + fib(n - 2),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn works() { assert_eq!(super::fib(10), 55); }
}
