#include <iostream>
#include <vector>
#include "vec.hpp"

int main() {
    std::vector<int> v{1, 2, 3};
    for (auto& x : v) std::cout << "值：" << x << '\n';
    return Vec2{1, 2}.dot({3, 4}) == 11 ? 0 : 1;
}
