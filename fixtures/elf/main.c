/* 让链接能过；真实 ECU 的 ELF 通常无 main，链接时不带 crt 起步，这里只是探针 */
float calib_touch(void);
float other_touch(void);
int main(void) { return (int)(calib_touch() + other_touch()); }
