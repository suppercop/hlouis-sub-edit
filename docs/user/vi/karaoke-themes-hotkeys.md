# Karaoke Templater, Giao diện Bảo vệ mắt & Phím tắt

## Karaoke Templater & Template Doctor

- **Template Doctor**: Kiểm tra cú pháp ASS và biểu thức Lua 5.1 trong template trước khi chạy, chỉ rõ dòng lỗi và cung cấp nút sửa tự động an toàn.
- **Karaoke Templater**: Tương thích `kara-templater.lua` của Aegisub 3.5.1 (`code`, `template`, `mixin`, các modifier `once`, `line`, `syl`, `char`, `furi`, `noblank`, `notext`, `keeptags`, `loop N`, `fxgroup`, `all`, hàm `retime()`, `relayer()`, `restyle()`, `maxloop()`, biến `$x`/`$y`/`$sstart`/`$send`, biểu thức `!expr!`) chạy trong sandbox LuaJIT an toàn (chặn `io`, `os`, `package.loadlib`; giới hạn RAM và vòng lặp vô hạn).
- Các dòng sinh ra được gắn `Effect = fx`, có thể xóa và tạo lại bất cứ lúc nào mà không làm mất dòng `karaoke` gốc.

## Giao diện & Chế độ Bảo vệ mắt (Eye Comfort)

Chuyển trực tiếp trên thanh menu hoặc trong **Cài đặt → Giao diện & Màu sắc**:
- **Sáng (Light)** và **Tối (Dark)** tối ưu độ tương phản khi làm sub lâu.
- **Bảo vệ mắt (Eye Comfort)**: Giảm ánh sáng xanh ấm dịu với thanh trượt cường độ **0–100%** và hẹn giờ tự động buổi tối.
- **Ngôn ngữ**: Chuyển **EN / VI** ngay lập tức không cần khởi động lại.

## Phím tắt & Bộ phím số Medusa

- Xử lý phím tắt theo ngữ cảnh giống Aegisub: `Always` → khung đang chọn (`Subtitle Grid`, `Subtitle Edit Box`, `Audio`, `Video`, `Styling Assistant`) → `Default`.
- Hỗ trợ nhập trực tiếp `hotkey.json` (bao gồm bộ phím số **Medusa** `KP_0`–`KP_9`, `KP_Enter`) và `config.json` từ Aegisub 3.5.1 trong **Cài đặt → Chung → Nhập cài đặt từ Aegisub...**.

## Khôi phục sau sự cố & Nhật ký hệ thống

- **Tự động lưu nguyên tử (Atomic Autosave)**: Ghi tệp tạm, gọi `fsync` rồi đổi tên nguyên tử — không bao giờ làm hỏng tệp `.ass` gốc nếu mất điện giữa chừng.
- **Khôi phục sau sự cố**: Tự phát hiện lần thoát bất thường, hiển thị danh sách bản lưu kèm thời gian, số dòng và so sánh diff với tệp trên đĩa.
- **Nhật ký hệ thống (Event Log)**: Lọc theo mức độ/module và xuất gói báo lỗi an toàn (mặc định ẩn đường dẫn cá nhân).
