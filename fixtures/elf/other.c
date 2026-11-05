/* 同名 static：与 calib.c 的 s_private_gain 冲突探测 */
static volatile float s_private_gain = 9.5f;

float other_touch(void) { return s_private_gain; }
