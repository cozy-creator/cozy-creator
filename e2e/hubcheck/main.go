// hubcheck sits between cozy and a Hub and holds every request and response to the Hub's API document, the one
// it serves at GET /openapi.yaml (tensorhub api/openapi.yaml: the server is the truth, the document follows it).
// Each mismatch is one JSON line in <out>/hub-mismatches.jsonl: a request that breaks the document is the client's
// drift, a response that breaks it the document's. A route the document lacks is a mismatch; an operation marked
// `x-raw-body` (pages, redirects, bytes) has its status checked, not its body. Every object the document gives
// properties takes no others, so an undocumented field is a mismatch too. <out>/hub-calls.json counts the calls
// of each operation.
//
//	hubcheck --hub https://tensorhub.com --out DIR [--listen 127.0.0.1:0] [--spec FILE]
//
// It prints the address it listens on, then serves until it is ended.
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"sync"

	"github.com/getkin/kin-openapi/openapi3"
	"github.com/getkin/kin-openapi/openapi3filter"
	"github.com/getkin/kin-openapi/routers"
)

// Mismatch is one place the traffic and the document disagree.
type Mismatch struct {
	Method string `json:"method"`
	Route  string `json:"route"`
	Path   string `json:"path"`
	Status int    `json:"status,omitempty"`
	Side   string `json:"side"`
	Field  string `json:"field,omitempty"`
	Error  string `json:"message"`
}

type checker struct {
	doc    *openapi3.T
	proxy  *httputil.ReverseProxy
	mu     sync.Mutex
	out    *os.File
	calls  map[string]int
	counts string
}

func main() {
	hub := flag.String("hub", "https://tensorhub.com", "the Hub")
	out := flag.String("out", "", "where mismatches and call counts go")
	listen := flag.String("listen", "127.0.0.1:0", "the address cozy calls instead of the Hub")
	spec := flag.String("spec", "", "an API document file instead of the Hub's own GET /openapi.yaml")
	flag.Parse()
	if *out == "" {
		fail(errors.New("--out is required"))
	}
	upstream, err := url.Parse(strings.TrimRight(*hub, "/"))
	if err != nil {
		fail(err)
	}
	doc, err := load(upstream, *spec)
	if err != nil {
		fail(err)
	}
	if err := os.MkdirAll(*out, 0o755); err != nil {
		fail(err)
	}
	file, err := os.Create(filepath.Join(*out, "hub-mismatches.jsonl"))
	if err != nil {
		fail(err)
	}
	c := &checker{doc: doc, out: file, calls: map[string]int{}, counts: filepath.Join(*out, "hub-calls.json")}
	c.proxy = &httputil.ReverseProxy{Rewrite: func(r *httputil.ProxyRequest) {
		r.SetURL(upstream)
		r.Out.Host = upstream.Host
		// The transport asks for gzip itself and hands back plain bytes to check.
		r.Out.Header.Del("Accept-Encoding")
	}}
	listener, err := net.Listen("tcp", *listen)
	if err != nil {
		fail(err)
	}
	fmt.Println(listener.Addr().String())
	fail(http.Serve(listener, c.mux()))
}

func fail(err error) {
	fmt.Fprintln(os.Stderr, "hubcheck:", err)
	os.Exit(1)
}

// load reads the document the Hub serves (or --spec), checks it is a valid OpenAPI document, and closes every
// object that lists properties to any other.
func load(hub *url.URL, file string) (*openapi3.T, error) {
	var raw []byte
	var err error
	if file != "" {
		raw, err = os.ReadFile(file)
	} else {
		var resp *http.Response
		if resp, err = http.Get(hub.String() + "/openapi.yaml"); err == nil {
			defer resp.Body.Close()
			if resp.StatusCode != http.StatusOK {
				return nil, fmt.Errorf("GET %s/openapi.yaml: %s", hub, resp.Status)
			}
			raw, err = io.ReadAll(resp.Body)
		}
	}
	if err != nil {
		return nil, err
	}
	doc, err := openapi3.NewLoader().LoadFromData(raw)
	if err != nil {
		return nil, err
	}
	if err := doc.Validate(context.Background()); err != nil {
		return nil, err
	}
	closed := map[*openapi3.Schema]bool{}
	var close func(*openapi3.SchemaRef)
	close = func(ref *openapi3.SchemaRef) {
		if ref == nil || ref.Value == nil || closed[ref.Value] {
			return
		}
		s := ref.Value
		closed[s] = true
		if len(s.Properties) > 0 && s.AdditionalProperties.Has == nil && s.AdditionalProperties.Schema == nil {
			no := false
			s.AdditionalProperties.Has = &no
		}
		for _, p := range s.Properties {
			close(p)
		}
		close(s.Items)
		close(s.AdditionalProperties.Schema)
		for _, list := range []openapi3.SchemaRefs{s.AllOf, s.OneOf, s.AnyOf} {
			for _, p := range list {
				close(p)
			}
		}
	}
	for _, s := range doc.Components.Schemas {
		close(s)
	}
	for _, item := range doc.Paths.Map() {
		for _, op := range item.Operations() {
			if op.RequestBody != nil && op.RequestBody.Value != nil {
				for _, media := range op.RequestBody.Value.Content {
					close(media.Schema)
				}
			}
			for _, resp := range op.Responses.Map() {
				if resp.Value != nil {
					for _, media := range resp.Value.Content {
						close(media.Schema)
					}
				}
			}
		}
	}
	return doc, nil
}

var rest = regexp.MustCompile(`\{(\w+)\}`)

// mux routes each request to its operation by Go's ServeMux, as the Hub's own router does: `/x/` is `/x/{$}`, an
// `x-rest` parameter `{p...}`. Whatever no operation claims is undocumented and still forwarded.
func (c *checker) mux() *http.ServeMux {
	mux := http.NewServeMux()
	for path, item := range c.doc.Paths.Map() {
		for method, op := range item.Operations() {
			pattern := rest.ReplaceAllStringFunc(path, func(p string) string {
				name := p[1 : len(p)-1]
				for _, param := range append(append(openapi3.Parameters{}, item.Parameters...), op.Parameters...) {
					if param.Value != nil && param.Value.In == "path" && param.Value.Name == name && param.Value.Extensions["x-rest"] == true {
						return "{" + name + "...}"
					}
				}
				return p
			})
			if strings.HasSuffix(pattern, "/") {
				pattern += "{$}"
			}
			route := &routers.Route{Spec: c.doc, Path: path, PathItem: item, Method: method, Operation: op}
			mux.HandleFunc(method+" "+pattern, func(w http.ResponseWriter, r *http.Request) { c.check(w, r, route) })
		}
	}
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		c.record(Mismatch{Method: r.Method, Path: r.URL.Path, Side: "request", Error: "no operation of the document serves this route"})
		recorder := &recorder{ResponseWriter: w}
		c.proxy.ServeHTTP(recorder, r)
		c.count("undocumented " + r.Method + " " + r.URL.Path)
	})
	return mux
}

func (c *checker) check(w http.ResponseWriter, r *http.Request, route *routers.Route) {
	op := route.Operation
	raw := op.Extensions["x-raw-body"] == true
	at := Mismatch{Method: r.Method, Route: route.Path, Path: r.URL.Path}
	params := map[string]string{}
	documented := map[string]bool{}
	for _, param := range append(append(openapi3.Parameters{}, route.PathItem.Parameters...), op.Parameters...) {
		if p := param.Value; p != nil {
			if p.In == "path" {
				params[p.Name] = r.PathValue(p.Name)
			}
			if p.In == "query" {
				documented[p.Name] = true
			}
		}
	}
	for name := range r.URL.Query() {
		if !documented[name] {
			c.mismatch(at, "request", "query."+name, errors.New("the document declares no such query parameter"))
		}
	}
	var body []byte
	if !raw && r.Body != nil {
		body, _ = io.ReadAll(r.Body)
		r.Body = io.NopCloser(bytes.NewReader(body))
	}
	input := &openapi3filter.RequestValidationInput{Request: r.Clone(r.Context()), PathParams: params, Route: route,
		Options: &openapi3filter.Options{AuthenticationFunc: openapi3filter.NoopAuthenticationFunc, MultiError: true,
			ExcludeRequestBody: raw, RejectWhenRequestBodyNotSpecified: true}}
	input.Request.Body = io.NopCloser(bytes.NewReader(body))
	if err := openapi3filter.ValidateRequest(r.Context(), input); err != nil {
		c.mismatch(at, "request", "", err)
	}
	recorder := &recorder{ResponseWriter: w, keep: !raw && r.Method != http.MethodHead}
	c.proxy.ServeHTTP(recorder, r)
	if r.Context().Err() != nil {
		// The client went away mid-call: the proxy's own 502 ("context canceled") is not the Hub's answer.
		return
	}
	at.Status = recorder.status
	c.count(r.Method + " " + route.Path)
	if op.Responses.Status(recorder.status) == nil && op.Responses.Default() == nil {
		c.mismatch(at, "response", "", fmt.Errorf("status %d is not documented", recorder.status))
		return
	}
	input.Request.Body = io.NopCloser(bytes.NewReader(body))
	err := openapi3filter.ValidateResponse(r.Context(), &openapi3filter.ResponseValidationInput{
		RequestValidationInput: input, Status: recorder.status, Header: recorder.Header(),
		Body:    io.NopCloser(bytes.NewReader(recorder.body.Bytes())),
		Options: &openapi3filter.Options{MultiError: true, IncludeResponseStatus: true, ExcludeResponseBody: !recorder.keep}})
	if err != nil {
		c.mismatch(at, "response", "", err)
	}
}

// mismatch records each leaf of a validation error, with the field it names.
func (c *checker) mismatch(at Mismatch, side, field string, err error) {
	at.Side, at.Field, at.Error = side, field, err.Error()
	switch e := err.(type) {
	case openapi3.MultiError:
		for _, leaf := range e {
			c.mismatch(at, side, field, leaf)
		}
		return
	case *openapi3filter.RequestError:
		switch {
		case e.Parameter != nil:
			field = e.Parameter.In + "." + e.Parameter.Name
		case e.RequestBody != nil:
			field = "body"
		}
		if e.Err != nil {
			c.mismatch(at, side, field, e.Err)
			return
		}
		at.Field, at.Error = field, e.Reason
	case *openapi3filter.ResponseError:
		if e.Err != nil {
			c.mismatch(at, side, "body", e.Err)
			return
		}
		at.Error = e.Reason
	case *openapi3.SchemaError:
		// allOf and its kin say only that a part failed: the part's own error says where, from the same root.
		if e.Origin != nil {
			c.mismatch(at, side, field, e.Origin)
			return
		}
		if pointer := e.JSONPointer(); len(pointer) > 0 {
			at.Field = field + "/" + strings.Join(pointer, "/")
		}
		at.Error = e.Reason
	default:
		if inner := errors.Unwrap(err); inner != nil {
			c.mismatch(at, side, field, inner)
			return
		}
	}
	c.record(at)
}

func (c *checker) record(m Mismatch) {
	line, _ := json.Marshal(m)
	c.mu.Lock()
	defer c.mu.Unlock()
	_, _ = c.out.Write(append(line, '\n'))
}

func (c *checker) count(operation string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.calls[operation]++
	counts, _ := json.MarshalIndent(c.calls, "", "  ")
	_ = os.WriteFile(c.counts, counts, 0o644)
}

// recorder passes the response through, keeping its status and (for a body it checks) its bytes.
type recorder struct {
	http.ResponseWriter
	keep   bool
	status int
	body   bytes.Buffer
}

func (r *recorder) WriteHeader(status int) {
	r.status = status
	r.ResponseWriter.WriteHeader(status)
}

func (r *recorder) Write(b []byte) (int, error) {
	if r.status == 0 {
		r.status = http.StatusOK
	}
	if r.keep {
		r.body.Write(b)
	}
	return r.ResponseWriter.Write(b)
}

func (r *recorder) Flush() {
	if f, ok := r.ResponseWriter.(http.Flusher); ok {
		f.Flush()
	}
}
