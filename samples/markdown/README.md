# Markdown 預覽測試：中文與數學

這是一段**中文段落**，含有 *斜體*、`行內程式碼` 與~~刪除線~~。行內公式 $E = mc^2$、$\sum_{i=1}^{n} i = \frac{n(n+1)}{2}$。

## 表格

| 語言 | 副檔名 | 對齊（右） |
| :--- | :----: | ---------: |
| C    | `.c`   | 1 |
| Rust | `.rs`  | 22 |
| 中文 | 無     | 333 |

## 巢狀清單與待辦

- 第一層
  - 第二層
    1. 第三層有序
    2. 第三層有序（二）
- 回到第一層

- [x] 已完成項目
- [ ] 未完成項目

> 引用區塊：「雲端只是別人的電腦。」
> > 巢狀引用

## 區塊公式

$$
\begin{aligned}
\nabla \cdot \mathbf{E} &= \frac{\rho}{\varepsilon_0} \\
\int_0^\infty e^{-x^2}\,dx &= \frac{\sqrt{\pi}}{2}
\end{aligned}
$$

## 程式碼（不應被當成公式）

```python
price = "$5 and $10"   # 這裡的 $ 不是公式
def hello(name: str) -> str:
    return f"你好，{name}"
```

```c
#include <stdio.h>
int main(void) { printf("$HOME\n"); return 0; }
```

```rust
fn main() { println!("{}", "雲端"); }
```

行內程式碼 `$PATH` 也不是公式。

## 圖片與連結

相對圖片（同一雲端目錄，需登入授權）：

![圖表](img/chart.png)

含空白與中文檔名的相對圖片：![小圖](<img/小 圖.png>)

外部圖片（預設不載入）：![tracker](https://example.com/pixel.png)

- [相對文件連結：另一份文件](docs/另一份文件.md)
- [外部連結](https://example.org)
- [危險連結](javascript:alert(1))

<script>alert('這段 HTML 不應執行')</script>
<img src="x" onerror="alert('xss')">
