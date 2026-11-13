import { useState } from 'react';

export function Counter({ start = 0 }: { start?: number }) {
  const [n, setN] = useState(start);
  return <button onClick={() => setN(n + 1)}>點擊次數：{n}</button>;
}
