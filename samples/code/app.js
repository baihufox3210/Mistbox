// JavaScript 範例
const greet = (name = '世界') => `你好，${name}！`;
export default async function main() {
  const res = await fetch('/api/v1/health');
  console.log(greet(), await res.json());
}
