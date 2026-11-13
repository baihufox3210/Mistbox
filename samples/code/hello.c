#include <stdio.h>
#include "util.h"

/* 中文註解：Tab 縮排測試 */
int main(void) {
	for (int i = 0; i < 3; i++) {
		printf("你好 %d\n", square(i));
	}
	return 0;
}
