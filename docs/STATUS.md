# Trạng thái dự án

Cập nhật lần cuối: 2026-11-27 12:55 (giờ VN)

## Người điều hành hiện tại

Claude (tài khoản chính)

## Đang chạy

- **PR #1 (`phase-0`)**: đã sửa theo review độc lập. Hai commit 47e7833 (code) và a27603a (license, report) đều có CI xanh, PR merge được. **Chưa merge**: chờ perf-budget rebase xong và test lại trên máy.
- **Nhánh `perf-budget`** (agent `agy`, chạy từ 11:56): giảm vẽ lại khi rảnh, GPU LowPower, cache highlight, benchmark Criterion và cổng CI chặn khi chậm đi quá 10%. Đã đẩy 5 commit, đang chờ CI.

## Bước kế tiếp

1. Agent perf-budget xong lượt hiện tại, thì chạy lượt `prompt-rebase-perf.txt` (model high): rebase `perf-budget` lên `origin/phase-0`, giữ thay đổi của cả hai phía, CI xanh. Không áp lại patch, không làm lại license hay report.
2. Mở PR cho `perf-budget`, đích là `phase-0` hoặc `main` (sau khi PR #1 merge).
3. Test lại trên máy với bản release: RAM private (mục tiêu < 600 MB, hiện 771 MB), CPU khi rảnh, phát 60 s trên máy rảnh (kiểm tra 107 frame rớt ở 36 s đầu), Loop, Ctrl+Z ở ô edit.
4. Báo chủ dự án merge PR #1 trước, perf-budget sau.
5. Viết đề bài Phase 1: golden test so với Aegisub 3.5.1, MKV nhiều track phụ đề (xem `PLAN.md`), cache index.

## Đang chờ chủ dự án

- (tùy chọn) Tắt UniKey, gõ thử Telex có sẵn của Windows trong ô edit, rồi báo kết quả.

## Đã quyết định

- UI theo Aegisub 3.2.2 cổ điển; ẩn cột Actor và Margin; panel đổi kích thước được và nhớ kích thước.
- FFmpeg LGPL 9.0.2, lưu bản sao ở release `native-deps-v1`. BestSource là mặc định từ Phase 4.
- Việc lặt vặt chạy `gemini-4-hydrogen-high`; thiết kế và việc khó chạy `xhigh`.
- Code logic thuần có thể giao Gemini 4 Neon web viết patch; `agy` chỉ ráp, build và test trên Windows.
