# Chỉnh sửa, Timing, Video/Audio & Công cụ

## Mở Video & Phụ đề (Hỗ trợ MKV trên 40 track)

- Kéo thả trực tiếp tệp `.ass`, `.srt`, `.mkv`, Keyframe hoặc Timecode vào cửa sổ chính, hoặc chọn **Tệp → Mở phụ đề từ video (MKV)...**.
- **Nhập MKV đa track**: Khắc phục hoàn toàn lỗi giới hạn 32 track của Aegisub 3.5.1 — liệt kê mọi track phụ đề (hỗ trợ 64+ track), hiển thị rõ codec (`ASS`, `SSA`, `SRT`, `WebVTT`; track ảnh `PGS`/`VobSub` hiển thị kèm lý do không chỉnh sửa trực tiếp), ngôn ngữ, tiêu đề, cờ Default/Forced, số dòng và xem trước 5 dòng đầu. Có thể lọc theo ngôn ngữ/từ khóa và chọn nhập nhiều track thành tài liệu riêng hoặc gộp chung.

## Bảng phụ đề & Ô chỉnh sửa

- **Tách biệt lịch sử hoàn tác theo từng dòng**: Con trỏ và Undo không bị lẫn khi chuyển dòng; gõ liên tục được gộp mượt không giật khung hình.
- **Chế độ Thời gian / Khung hình**: Chuyển nhanh giữa `H:MM:SS.cc` và số khung hình khi đã nạp Timecode video.
- **Cảnh báo CPS**: Cột CPS tự đổi màu khi tốc độ đọc quá nhanh để dễ dàng QC.

## Timing, Âm thanh & Phổ tần (Spectrum)

- Chuyển nhanh giữa **Waveform** và **Spectrum** (chất lượng FFT `Regular`, `Better`, `High`, `Insane`).
- Kéo mốc bắt đầu/kết thúc trực tiếp trên âm thanh, bám Keyframe, hoặc dùng **Timing Post-Processor (TPP)** và **Dịch thời gian (Shift Times)**.

## Style & Công cụ tích hợp

- **Trình quản lý Style**: Chỉnh sửa `.sty`, nhập Style từ tệp `.ass` khác và tự động cập nhật khi đổi tên Style.
- **Đổi độ phân giải (Resample)**: Tự động quy đổi tọa độ, cỡ chữ, viền, bóng và vector `\clip` giữa các độ phân giải (ví dụ 720p ↔ 1080p).
- **HLouis Tools**:
  - **Chọn theo Style** (`Ctrl+Alt+S` chọn nhanh / `Ctrl+Alt+Shift+S` mở hộp thoại): Chọn tất cả dòng cùng Style hoặc chọn nhiều Style kèm số lượng dòng.
  - **Dọn khoảng trắng & xuống dòng thừa** (`Ctrl+Alt+W`): Chuẩn hóa khoảng trắng Unicode, gộp `\N` thừa, tự động bỏ qua dòng vẽ vector `\p1+` và cho xem trước diff trước khi áp dụng.
