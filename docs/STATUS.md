# Trạng thái dự án

Cập nhật lần cuối: 2026-11-27 13:40 (giờ VN)

## Người điều hành hiện tại

Trống (Claude chính sắp hết hạn mức 5 giờ, Claude thứ hai có thể tiếp quản)

## Đang chạy

- **PR #1 (`phase-0`)**: đã sửa theo review (47e7833, a27603a), CI xanh, merge được. **Chưa merge.**
- **PR #2 (`perf-budget`)**: CI xanh. RAM private khoảng 360–400 MiB (trước là 921 MB), hết vẽ lại khi rảnh, benchmark Criterion và cổng CI chặn khi chậm đi quá 10%. Số liệu do agent tự đo. **Agent đang rebase PR #2 lên phase-0 mới** (model high).
- **Nhánh `phase-1`** (76c0818): crate `ass-core` (do Gemini Neon web viết, Claude áp và sửa). Đọc rồi ghi file golden-ass thật (6.865 dòng do Aegisub 3.5.1 lưu) ra giống hệt từng byte. Đọc 20k dòng 6,9 ms, ghi 5 ms, undo 53 µs. **Agent KHÔNG áp lại ass-core.patch.**
- **Gemini Neon web**: một lượt lớn, 5 patch áp theo thứ tự: 1 `ass-core-followups` (CRLF/LF lẫn lộn, Windows-1258 câu ngắn, che raw_line, fixture dạng văn bản), 2 `ass-tools` (Shift Times, Find & Replace, Select Lines, Join/Split, TPP, Resample, karaoke split/join), 3 `keyframes`, 4 `fonts-check`, 5 `mkv-subs` (đọc phụ đề MKV không giới hạn track; test thật bằng `HLOUIS_MKV_MULTISUB` trỏ tới Fool.Night E02, 40 track, phải xong dưới 3 s). Đề bài 1–4: `gemini-web-review\PROMPT-PHASE1-FOLLOWUP.txt`; đề bài 5 trong thread dự án.

## Bước kế tiếp

1. Rebase PR #2 xong và CI xanh: test lại trên máy bằng bản release (RAM private, CPU khi rảnh, phát 60 s trên máy rảnh, Loop, Ctrl+Z ở ô edit), so với số agent đo.
2. Báo chủ dự án merge PR #1 trước, rồi PR #2.
3. Khi có patch của Neon: áp vào `phase-1` trong worktree riêng (patch tải về có thể bị đổi ký tự xuống dòng thành CRLF; chỉ bỏ CR ở các file code, giữ CRLF trong `tests/fixtures`). Chạy fmt, clippy, test, golden test (`HLOUIS_GOLDEN_DIR=Downloads\Claude Code\golden-ass`), test MKV với file Fool.Night E02 (40 track), rồi push lên `phase-1`.
4. Phần còn lại của Phase 1 cho agent: autosave và nối ass-core/mkv-subs vào app (mở, lưu, chọn track MKV).
5. Golden-ass: hiện có 1 file. Có thể thêm file karaoke, file có font nhúng hoặc vector, và một file nhỏ.

## Đang chờ chủ dự án

- Merge PR #1 rồi PR #2 khi Claude báo sẵn sàng.
- (tùy chọn) Gõ thử Telex có sẵn của Windows trong ô edit (đã tắt UniKey).

## Đã quyết định

- UI theo Aegisub 3.2.2 cổ điển; ẩn cột Actor và Margin; panel đổi kích thước được và nhớ kích thước.
- FFmpeg LGPL 9.0.2, lưu bản sao ở release `native-deps-v1`. BestSource là mặc định từ Phase 4.
- Việc lặt vặt chạy `gemini-4-hydrogen-high`; thiết kế và việc khó chạy `xhigh`.
- Code logic thuần có thể giao Gemini 4 Neon web viết patch; `agy` chỉ ráp, build và test trên Windows.
