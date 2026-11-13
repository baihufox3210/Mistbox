#!/usr/bin/env python3
"""Docstring：示範 Python 高亮。"""
from dataclasses import dataclass


@dataclass
class Item:
    name: str
    price: float = 0.0


def total(items: list[Item]) -> float:
    return sum(i.price for i in items)  # 計算總和 🧮


if __name__ == "__main__":
    print(total([Item("咖啡", 80), Item("蛋糕", 120.5)]))
