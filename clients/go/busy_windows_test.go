//go:build windows

package inferd

import "testing"

// TestContainsErrPipeBusy pins the #49 fix: the busy-pipe match must be
// case-insensitive so it catches the *capitalised* text Windows actually
// returns ("All pipe instances are busy."). Before the fix the matcher
// looked only for the lower-case form, so DialPipe's retry never fired
// and a busy pipe failed immediately.
func TestContainsErrPipeBusy(t *testing.T) {
	cases := []struct {
		name string
		s    string
		want bool
	}{
		{
			// The exact string os.OpenFile surfaces on a busy pipe,
			// including the DialPipe %w wrapping — capital "All".
			name: "windows capitalised busy (the #49 regression)",
			s:    `open pipe \\.\pipe\inferd: open \\.\pipe\inferd: All pipe instances are busy.`,
			want: true,
		},
		{name: "lower-case busy", s: "all pipe instances are busy", want: true},
		{name: "mixed-case busy", s: "AlL pIpE iNsTaNcEs ArE bUsY", want: true},
		{name: "system cannot find the file (capitalised)", s: "The system cannot find the file specified.", want: true},
		{name: "access is denied (capitalised)", s: "Access is denied.", want: true},
		{name: "unrelated error", s: "connection reset by peer", want: false},
		{name: "empty", s: "", want: false},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := containsErrPipeBusy(c.s); got != c.want {
				t.Errorf("containsErrPipeBusy(%q) = %v, want %v", c.s, got, c.want)
			}
		})
	}
}
