-- SQL 範例
SELECT n.name, COUNT(*) AS 數量
FROM nodes AS n
WHERE n.kind = 'file' AND n.size > 1024
GROUP BY n.name
ORDER BY 數量 DESC;
