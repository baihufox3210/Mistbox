#pragma once
struct Vec2 {
    double x, y;
    constexpr double dot(const Vec2& o) const noexcept { return x * o.x + y * o.y; }
};
