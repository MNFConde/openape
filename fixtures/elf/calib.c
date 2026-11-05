/* M0-4 探针：覆盖标定/观测典型形态，看 a2ltool 的 DWARF 逻辑能取到哪些字段 */
#include <stdint.h>

typedef enum { MODE_IDLE = 0, MODE_RUN = 1, MODE_FAULT = 2 } mode_t;

typedef struct {
    float kp;
    float ki;
} pid_gains_t;

struct bitset_t {
    unsigned a : 3;
    unsigned b : 5;
    unsigned c : 8;
};

/* 标定量：FLASH 常量 */
const float cal_kp = 1.5f;
const float cal_map[4][3] = { {1, 2, 3}, {4, 5, 6}, {7, 8, 9}, {10, 11, 12} };
const float cal_axis_x[3] = { 0.0f, 0.5f, 1.0f };
const float cal_axis_y[4] = { 0.0f, 1.0f, 2.0f, 3.0f };
const pid_gains_t cal_gains = { 0.8f, 0.05f };
const mode_t cal_mode = MODE_RUN;

/* 观测量：RAM，volatile 防止被优化掉 */
volatile float obs_speed;
volatile float obs_rpm;
volatile uint16_t obs_flags;
volatile struct bitset_t obs_bits;
struct {
    float cur;
    float ref;
} obs_state;

/* static：本 CU 私有，other.c 里有同名者 */
static volatile float s_private_gain = 2.5f;

static float use_private(void) { return s_private_gain; }

/* 对外可见的 const，-O2 下仍会保留实体 */
const float cal_folded = 3.25f;

/* 仅本文件函数内使用的 static const，-O2 下可能被完全折叠 */
static const float s_folded = 7.75f;
static float use_folded(void) { return s_folded * 3.0f; }

float calib_touch(void) {
    return cal_kp + cal_map[0][0] + cal_axis_x[0] + cal_folded * 2.0f + use_folded() + use_private() + obs_speed;
}
