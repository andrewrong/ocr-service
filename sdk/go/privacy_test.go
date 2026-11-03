package ocrclient

import (
	"bufio"
	"bytes"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"testing"
)

// Kinds of private data. A finding names only one of these words, never the
// matched text: the CI logs of a public repository are public.
const (
	kindPath    = "path"
	kindEmail   = "email"
	kindTailnet = "tailnet"
)

const (
	maxScanBytes = 1 << 20
	nulProbe     = 8000
)

var (
	// The segment after the prefix is captured so the allow list can name it.
	pathRe = regexp.MustCompile(`/(?:Users|home|Volumes)/([^/\s"'` + "`" + `]+)/`)
	// Subdomains are included; a domain must not continue after the listed one.
	emailRe = regexp.MustCompile(`(?i)@(?:[a-z0-9-]+\.)*` +
		`(?:gmail\.com|googlemail\.com|126\.com|163\.com|qq\.com|foxmail\.com|outlook\.com|` +
		`hotmail\.com|live\.com|icloud\.com|me\.com|yahoo\.com|sina\.com|aliyun\.com)` +
		`(?:$|[^a-z0-9-])`)
	tailnetRe = regexp.MustCompile(`(?i)([a-z0-9-]+(?:\.[a-z0-9-]+)*)\.ts\.net(?:$|[^a-z0-9-])`)
)

// allowList holds path segments allowed after a local-path prefix and the
// (file, kind) pairs allowed as a whole.
type allowList struct {
	segments map[string]bool
	files    map[string]bool // "<file>:<kind>"
}

func (a allowList) fileAllows(file, kind string) bool {
	return a.files[file+":"+kind]
}

// parseAllow reads the allow file. Every entry must have a comment on the line
// above it, so that each exception says why it is not private data.
func parseAllow(text string) (allowList, error) {
	a := allowList{segments: map[string]bool{}, files: map[string]bool{}}
	prevComment := false
	sc := bufio.NewScanner(strings.NewReader(text))
	for n := 1; sc.Scan(); n++ {
		line := strings.TrimSpace(sc.Text())
		switch {
		case line == "":
			prevComment = false
		case strings.HasPrefix(line, "#"):
			prevComment = true
		default:
			if !prevComment {
				return a, fmt.Errorf("allow entry on line %d has no comment above it", n)
			}
			if i := strings.LastIndex(line, ":"); i > 0 {
				switch line[i+1:] {
				case kindPath, kindEmail, kindTailnet:
					a.files[line] = true
				default:
					return a, fmt.Errorf("allow entry on line %d has an unknown kind", n)
				}
			} else {
				a.segments[line] = true
			}
			prevComment = false
		}
	}
	return a, sc.Err()
}

// matchLine returns the kinds of private data found in one line, each at most
// once, in a fixed order. Path segments in the allow list do not count.
func matchLine(line string, a allowList) []string {
	var kinds []string
	for _, m := range pathRe.FindAllStringSubmatch(line, -1) {
		if !a.segments[m[1]] {
			kinds = append(kinds, kindPath)
			break
		}
	}
	if emailRe.MatchString(line) {
		kinds = append(kinds, kindEmail)
	}
	for _, m := range tailnetRe.FindAllStringSubmatch(line, -1) {
		host := strings.ToLower(m[1])
		if host != "example" && !strings.HasSuffix(host, ".example") {
			kinds = append(kinds, kindTailnet)
			break
		}
	}
	return kinds
}

// scanRepo lists the tracked files of the git repository holding dir and
// returns one "file:line: kind" message per finding that is not allowed. Files
// in exclude (root-relative, slash separated) are skipped.
func scanRepo(dir string, a allowList, exclude map[string]bool) ([]string, error) {
	root, err := repoRoot(dir)
	if err != nil {
		return nil, err
	}
	out, err := exec.Command("git", "-C", root, "ls-files", "-z").Output()
	if err != nil {
		return nil, err
	}
	var findings []string
	for _, name := range strings.Split(string(out), "\x00") {
		if name == "" || exclude[name] {
			continue
		}
		data, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(name)))
		if err != nil {
			continue // a tracked file deleted in the working tree
		}
		if len(data) > maxScanBytes || bytes.IndexByte(data[:min(len(data), nulProbe)], 0) >= 0 {
			continue
		}
		for i, line := range strings.Split(string(data), "\n") {
			for _, kind := range matchLine(line, a) {
				if !a.fileAllows(name, kind) {
					findings = append(findings, fmt.Sprintf("%s:%d: %s", name, i+1, kind))
				}
			}
		}
	}
	sort.Strings(findings)
	return findings, nil
}

func repoRoot(dir string) (string, error) {
	cmd := exec.Command("git", "rev-parse", "--show-toplevel")
	cmd.Dir = dir
	out, err := cmd.Output()
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(string(out)), nil
}

// ownFiles names this test and its allow file relative to the repository root.
func ownFiles(t *testing.T) map[string]bool {
	t.Helper()
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	wd, err = filepath.EvalSymlinks(wd)
	if err != nil {
		t.Fatal(err)
	}
	root, err := repoRoot(wd)
	if err != nil {
		t.Fatal(err)
	}
	root, err = filepath.EvalSymlinks(root)
	if err != nil {
		t.Fatal(err)
	}
	rel, err := filepath.Rel(root, wd)
	if err != nil {
		t.Fatal(err)
	}
	return map[string]bool{
		filepath.ToSlash(filepath.Join(rel, "privacy_test.go")):               true,
		filepath.ToSlash(filepath.Join(rel, "testdata", "privacy_allow.txt")): true,
	}
}

func loadAllow(t *testing.T) allowList {
	t.Helper()
	text, err := os.ReadFile(filepath.Join("testdata", "privacy_allow.txt"))
	if err != nil {
		t.Fatal(err)
	}
	a, err := parseAllow(string(text))
	if err != nil {
		t.Fatal(err)
	}
	return a
}

func TestNoPrivateDataInTrackedFiles(t *testing.T) {
	findings, err := scanRepo(".", loadAllow(t), ownFiles(t))
	if err != nil {
		t.Fatal(err)
	}
	for _, f := range findings {
		t.Error(f)
	}
}

func TestPrivacyPatterns(t *testing.T) {
	allow := allowList{segments: map[string]bool{"me": true}, files: map[string]bool{}}
	cases := []struct {
		name string
		line string
		want []string
	}{
		{"users path", "/Us" + "ers/alice/proj", []string{kindPath}},
		{"home path", "/ho" + "me/bob/x", []string{kindPath}},
		{"volumes path", "/Vol" + "umes/data/x", []string{kindPath}},
		{"gmail", "someone" + "@gmail.com", []string{kindEmail}},
		{"126", "someone" + "@126.com", []string{kindEmail}},
		{"tailnet", "mac.tail1234" + ".ts.net", []string{kindTailnet}},
		{"allowed segment", "/Us" + "ers/me/x", nil},
		{"web route", "/us" + "ers/alice/x", nil},
		{"example e-mail", "a" + "@example.com", nil},
		{"anthropic e-mail", "noreply" + "@anthropic.com", nil},
		{"github noreply", "123+x" + "@users.noreply.github.com", nil},
		{"example tailnet", "desk.example" + ".ts.net", nil},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got := matchLine(c.line, allow)
			if strings.Join(got, ",") != strings.Join(c.want, ",") {
				t.Errorf("got kinds %v, want %v", got, c.want)
			}
		})
	}
}

func TestParseAllowRequiresComment(t *testing.T) {
	if _, err := parseAllow("me\n"); err == nil {
		t.Error("entry without a comment was accepted")
	}
	a, err := parseAllow("# placeholder\nme\n# fixture\ndocs/a.md:email\n")
	if err != nil {
		t.Fatal(err)
	}
	if !a.segments["me"] || !a.fileAllows("docs/a.md", kindEmail) || a.fileAllows("docs/a.md", kindPath) {
		t.Errorf("unexpected allow list %+v", a)
	}
}

// The matching file lies outside the directory the scan starts from, as
// sdk/go does for this repository.
func TestScanRepoFindsFileOutsideStartDirectory(t *testing.T) {
	repo := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", append([]string{"-C", repo}, args...)...)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-q")
	sub := filepath.Join(repo, "sdk", "go")
	if err := os.MkdirAll(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repo, "docs.md"), []byte("ok\nsee /Us"+"ers/alice/x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(sub, "own.txt"), []byte("/Us"+"ers/bob/x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(repo, "bin.dat"), []byte("\x00/Us"+"ers/carol/x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	got, err := scanRepo(sub, allowList{segments: map[string]bool{}, files: map[string]bool{}},
		map[string]bool{"sdk/go/own.txt": true})
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 1 || got[0] != "docs.md:2: path" {
		t.Errorf("got %v, want [docs.md:2: path]", got)
	}
}
