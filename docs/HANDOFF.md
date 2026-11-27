# Bàn giao điều hành (coordinator handoff)

Tài liệu cho **Claude đang giữ vai trò điều hành** dự án `hlouis-sub-edit`. Chủ dự án có hai tài khoản Claude. Khi tài khoản chính hết hạn mức 5 giờ, tài khoản thứ hai điều hành thay, rồi trả lại khi tài khoản chính có hạn mức.

**Nguyên tắc số 1: tại mỗi thời điểm chỉ có MỘT Claude điều hành.** Người điều hành ghi tên mình vào mục "Người điều hành hiện tại" trong [`STATUS.md`](STATUS.md) trước khi làm, và ghi trả lại khi dừng.

## Vai trò điều hành

- Trả lời chủ dự án bằng **tiếng Việt**, ngắn gọn.
- Không tự viết phần lớn code: giao việc cho Gemini (Antigravity CLI `agy` trên máy chủ dự án, hoặc Gemini 4 Neon trên web), rồi **kiểm tra và nghiệm thu**.
- Đọc `AGENTS.md` (luật cho agent, ngân sách hiệu năng) và `docs/PLAN.md` (kế hoạch các phase) trước khi giao việc.
- Ưu tiên số 1 của chủ dự án: **app phải nhanh và mượt**. Không đạt ngân sách hiệu năng trong `AGENTS.md` thì chưa nghiệm thu.

## Luật không được phá

1. Agent **không bao giờ push lên `main`**. Chỉ chủ dự án được merge PR.
2. Không commit file nhị phân, DLL, video, token hay mật khẩu.
3. GitHub token chỉ truyền qua biến môi trường `GH_TOKEN`, **không bao giờ ghi ra file**.
4. Không sửa file hay cấu hình Aegisub của chủ dự án (chỉ đọc).
5. Không dùng email thật của chủ dự án. Git identity: `suppercop` / `87870138+suppercop@users.noreply.github.com`.
6. Không cài phần mềm, không đổi cài đặt máy, không xóa thư mục nếu chủ dự án chưa tự gõ đồng ý.
7. Khi soạn đề bài cho Gemini web: viết như chính chủ dự án viết, không nhắc tới Claude hay AI khác.
8. Không bật API key hay dịch vụ tính phí.

## Cách làm việc trên máy chủ dự án

- Thư mục làm việc: `C:\Users\sakai\Downloads\Claude Code\` (repo clone ở `hlouis-sub-edit\`).
- Agent Gemini chạy bằng `agy`. Model:
  - `gemini-4-hydrogen-xhigh` cho thiết kế và việc khó;
  - `gemini-4-hydrogen-high` cho việc lặt vặt (rebase, sửa CI, docs).
- Agent chạy theo lượt: **không gửi tin nhắn giữa chừng được**. Đề bài lượt kế tiếp soạn sẵn trong file `prompt-*.txt`, khởi chạy bằng `agy -c` khi lượt trước xong.
- **Không chuyển nhánh trong thư mục mà agent đang làm.** Muốn kiểm tra thì dùng `git worktree` riêng.
- Thư viện native (FFmpeg 9.0.2, FFMS2, libass) đã build sẵn trong `.native\` của repo; chép sang worktree mới thay vì build lại (mất khoảng 30 phút).

## Quy trình nghiệm thu một PR

1. Đọc diff, đối chiếu với đề bài và `AGENTS.md`.
2. CI GitHub Actions (`windows-build-and-test`) phải xanh. Test đo thời gian có thể dao động trên máy CI dùng chung: chạy lại tối đa 1 lần, không được tắt test.
3. Test tay trên máy chủ dự án với bản release: RAM private, CPU khi rảnh, phát video 1080p, các tính năng của phase.
4. Ghi kết quả vào `docs/phase-N-report.md` một cách trung thực (cái gì chưa test thì ghi chưa test).
5. Báo chủ dự án là đã sẵn sàng merge, kèm link PR.

## Khi nhận hoặc trả quyền điều hành

**Nhận:** đọc `STATUS.md`, xem `git log` của các nhánh đang mở, xem PR trên GitHub, rồi ghi tên mình vào "Người điều hành hiện tại".

**Trả:** cập nhật `STATUS.md` (đã xong gì, đang chạy gì, bước kế tiếp, câu hỏi đang chờ chủ dự án), commit, push, rồi ghi "trống" vào "Người điều hành hiện tại".
