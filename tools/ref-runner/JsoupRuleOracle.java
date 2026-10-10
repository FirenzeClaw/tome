import java.io.IOException;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.stream.Collectors;
import java.util.stream.Stream;

import org.jsoup.Jsoup;
import org.jsoup.nodes.Document;
import org.jsoup.nodes.Element;
import org.jsoup.select.Elements;
import org.jsoup.select.Selector;

/**
 * 规则求值 oracle：用 <b>Jsoup 的公开 API</b> 为一批合成用例产出参照输出。
 *
 * <h2>干净室声明</h2>
 *
 * 本文件**只使用 Jsoup 的公开 API**（{@code Jsoup.parse}、{@code Element.select}），
 * **未读取、未移植** {@code D:/Code/legado} 的任何 Kotlin/Java 源码。
 * 求值与序列化约定由本项目自行定义（本类注释 + {@code tools/ref-runner/CLEANROOM.md}）。
 * 与 S0 的 {@code spikes/s0/tools/JsoupDomDump.java} 同一做法。
 *
 * <h2>它在差分测试中的位置</h2>
 *
 * tome 的规则引擎对齐目标是 Jsoup 的 Selector 行为。本 oracle 直接驱动
 * Jsoup 本身，故它是"参照行为"的一手来源——比"照抄某实现"更硬，
 * 因为它是**运行**出来的而不是**读**出来的。
 *
 * <h2>输入与输出约定</h2>
 *
 * 输入：与 tome 语料同构的 JSON 用例数组（{@code name}/{@code kind}/{@code input}/
 * {@code rule}），目录由命令行给出。为了不引 JSON 库，只解析本项目自己产出的
 * **扁平 JSON 用例文件**（见 {@code readCases}）。
 *
 * 输出：{@code <out-dir>/jsoup-oracle.json}，每条含
 * {@code name} / {@code expected}（oracle 结果）/ {@code error}。
 *
 * 之所以自行解析而不引 Jackson/Gson：多一个依赖就多一份版本漂移，
 * 而对照数据的价值恰恰在于"两侧除被测行为外别无可变量"。
 */
public final class JsoupRuleOracle {

    /** 命令行参数：输出目录。 */
    public static void main(String[] args) throws IOException {
        if (args.length < 1) {
            System.err.println("用法：JsoupRuleOracle <out-dir> [case-file...]");
            System.exit(2);
        }

        Path outDir = Paths.get(args[0]);
        Files.createDirectories(outDir);

        List<Path> caseFiles = new ArrayList<>();
        if (args.length > 1) {
            for (int i = 1; i < args.length; i++) {
                caseFiles.add(Paths.get(args[i]));
            }
        } else {
            // 默认取仓库自带的语料（与 tome 的 verify --corpus 同一批输入）
            Path corpus = Paths.get("../..", "tests", "corpus").normalize();
            if (Files.isDirectory(corpus)) {
                try (Stream<Path> stream = Files.list(corpus)) {
                    stream.filter(p -> p.toString().endsWith(".json"))
                          .sorted()
                          .forEach(caseFiles::add);
                }
            }
        }

        if (caseFiles.isEmpty()) {
            System.err.println("未找到用例文件");
            System.exit(2);
        }

        StringBuilder json = new StringBuilder();
        json.append("{\n  \"oracle\": \"jsoup\",\n");
        json.append("  \"jsoup_version\": \"")
            .append(escape(Jsoup.class.getPackage() == null
                    ? "unknown" : Jsoup.class.getPackage().getImplementationVersion()))
            .append("\",\n");
        json.append("  \"note\": \"由 Jsoup 公开 API 直接求值产出；干净室边界见 tools/ref-runner/CLEANROOM.md\",\n");
        json.append("  \"cases\": [\n");

        int total = 0;
        boolean first = true;
        for (Path file : caseFiles) {
            List<Case> cases;
            try {
                cases = readCases(Files.readString(file, StandardCharsets.UTF_8));
            } catch (RuntimeException error) {
                System.err.println("[跳过] " + file + "：" + error.getMessage());
                continue;
            }
            for (Case one : cases) {
                Result result = evaluate(one);
                if (!first) {
                    json.append(",\n");
                }
                first = false;
                json.append("    {")
                    .append("\"name\": ").append(quote(one.name)).append(", ")
                    .append("\"source_file\": ").append(quote(file.getFileName().toString())).append(", ")
                    .append("\"rule\": ").append(quote(one.rule)).append(", ")
                    .append("\"expected\": ").append(stringArray(result.values)).append(", ")
                    .append("\"error\": ").append(result.error == null ? "null" : quote(result.error))
                    .append("}");
                total++;
            }
        }
        json.append("\n  ],\n  \"total\": ").append(total).append("\n}\n");

        Path out = outDir.resolve("jsoup-oracle.json");
        Files.writeString(out, json.toString(), StandardCharsets.UTF_8);
        System.out.println("已写出 " + total + " 条 oracle 结果 → " + out);
    }

    // -----------------------------------------------------------------------
    // 求值：只走 Jsoup 公开 API
    // -----------------------------------------------------------------------

    /**
     * 求值一条用例。
     *
     * <p>只支持 HTML 类用例：JSONPath / XPath / JS 不属于 Jsoup 的能力面，
     * 对本 oracle 而言是"没有参照行为可言"，而不是"求值失败"。
     * 把它们明确标成 error 而不是空结果——空结果会被误读成"参照说应该是空的"。
     */
    private static Result evaluate(Case one) {
        if (!"html".equalsIgnoreCase(one.kind)) {
            return Result.failure("oracle 不覆盖 kind=" + one.kind + "（非 Jsoup 能力面）");
        }
        try {
            Document document = Jsoup.parse(one.input);
            // 反序标记：与书源规则约定一致（规则前加 `-`）
            String rule = one.reversed ? one.rule.substring(1) : one.rule;

            List<String> values = runRule(document, rule);
            if (one.reversed) {
                List<String> reversed = new ArrayList<>(values);
                java.util.Collections.reverse(reversed);
                values = reversed;
            }
            return Result.success(values);
        } catch (Selector.SelectorParseException | IllegalArgumentException error) {
            return Result.failure("规则求值失败：" + error.getMessage());
        }
    }

    /**
     * 求一条 Jsoup 规则，产出结果列表。
     *
     * <p>支持本项目语料实际用到的后缀面：
     * {@code @text} / {@code @ownText} / {@code @textNodes} / {@code @html} /
     * {@code @outerHtml} / {@code @href} 等属性取值。
     * 未识别的后缀**明确报错**而非猜——猜出来的"参照值"会毒化整批对照数据。
     */
    private static List<String> runRule(Document document, String rawRule) {
        String rule = rawRule.trim();
        // 去掉 tome 的引擎前缀（@css: / @@），Jsoup 侧不需要它们
        if (rule.startsWith("@css:")) {
            rule = rule.substring(5).trim();
        } else if (rule.startsWith("@@")) {
            rule = rule.substring(2).trim();
        }

        String selector = rule;
        String suffix = null;
        // 取**最后**一个 `@`：属性名里不含 `@`，而选择器里可能含（如 [data-x@y]）
        int at = rule.lastIndexOf('@');
        if (at > 0) {
            selector = rule.substring(0, at).trim();
            suffix = rule.substring(at + 1).trim();
        }

        Elements elements = document.select(selector);
        List<String> out = new ArrayList<>();
        // 无后缀时，Jsoup 的 select 结果以 outerHtml 呈现（与 `@outerHtml` 同）
        for (Element element : elements) {
            out.add(applySuffix(element, suffix));
        }
        return out;
    }

    /** 应用后缀取值器。未识别的后缀抛异常（不猜）。 */
    private static String applySuffix(Element element, String suffix) {
        if (suffix == null || suffix.isEmpty() || "outerHtml".equals(suffix)) {
            return element.outerHtml();
        }
        switch (suffix) {
            case "text":
                return element.text();
            case "ownText":
                return element.ownText();
            case "textNodes":
                return element.textNodes().stream()
                        .map(node -> node.getWholeText())
                        .collect(Collectors.joining("\n"));
            case "html":
                return element.html();
            case "allText":
                return element.wholeText();
            case "tagName":
                return element.tagName();
            case "className":
                return element.className();
            case "id":
                return element.id();
            case "val":
                return element.val();
            default:
                // 其余按属性名取（href/src/title/alt/data-* ...）
                if (element.hasAttr(suffix)) {
                    return element.attr(suffix);
                }
                throw new IllegalArgumentException("oracle 不识别后缀 @" + suffix);
        }
    }

    // -----------------------------------------------------------------------
    // 极简 JSON 读取（只认本项目语料的扁平结构）
    // -----------------------------------------------------------------------

    /** 一条用例。 */
    private static final class Case {
        String name;
        String kind;
        String input;
        String rule;
        boolean reversed;
    }

    /**
     * 解析用例数组。
     *
     * <p>刻意只支持**扁平对象数组 + 字符串/布尔字段**：语料格式就是这么定的
     * （见 {@code tests/corpus/README.md}）。多引一个 JSON 库会引入版本变量，
     * 而对照数据的用途要求"两侧除被测行为外别无可变量"。
     */
    private static List<Case> readCases(String json) {
        List<Case> cases = new ArrayList<>();
        int index = 0;
        while (true) {
            int open = json.indexOf('{', index);
            if (open < 0) {
                break;
            }
            int close = matchBrace(json, open);
            if (close < 0) {
                throw new IllegalArgumentException("JSON 花括号不闭合");
            }
            String body = json.substring(open + 1, close);
            Case one = new Case();
            one.name = strField(body, "name");
            one.kind = strField(body, "kind");
            one.input = strField(body, "input");
            one.rule = strField(body, "rule");
            one.reversed = boolField(body, "reversed");
            if (one.name != null && one.rule != null) {
                cases.add(one);
            }
            index = close + 1;
        }
        return cases;
    }

    /** 找与 open 配对的 `}`，跳过字符串字面量内的括号。 */
    private static int matchBrace(String text, int open) {
        int depth = 0;
        boolean inString = false;
        boolean escaped = false;
        for (int i = open; i < text.length(); i++) {
            char c = text.charAt(i);
            if (escaped) {
                escaped = false;
                continue;
            }
            if (c == '\\') {
                escaped = true;
                continue;
            }
            if (c == '"') {
                inString = !inString;
                continue;
            }
            if (inString) {
                continue;
            }
            if (c == '{') {
                depth++;
            } else if (c == '}') {
                depth--;
                if (depth == 0) {
                    return i;
                }
            }
        }
        return -1;
    }

    /** 取字符串字段的值（含转义还原）。字段不存在时返回 null。 */
    private static String strField(String body, String key) {
        String needle = "\"" + key + "\"";
        int at = body.indexOf(needle);
        if (at < 0) {
            return null;
        }
        int colon = body.indexOf(':', at + needle.length());
        if (colon < 0) {
            return null;
        }
        int start = colon + 1;
        while (start < body.length() && Character.isWhitespace(body.charAt(start))) {
            start++;
        }
        if (start >= body.length() || body.charAt(start) != '"') {
            return null;
        }
        StringBuilder value = new StringBuilder();
        boolean escaped = false;
        for (int i = start + 1; i < body.length(); i++) {
            char c = body.charAt(i);
            if (escaped) {
                switch (c) {
                    case 'n': value.append('\n'); break;
                    case 't': value.append('\t'); break;
                    case 'r': value.append('\r'); break;
                    case 'b': value.append('\b'); break;
                    case 'f': value.append('\f'); break;
                    case 'u':
                        if (i + 4 < body.length()) {
                            value.append((char) Integer.parseInt(body.substring(i + 1, i + 5), 16));
                            i += 4;
                        }
                        break;
                    default: value.append(c);
                }
                escaped = false;
                continue;
            }
            if (c == '\\') {
                escaped = true;
                continue;
            }
            if (c == '"') {
                return value.toString();
            }
            value.append(c);
        }
        return null;
    }

    /** 取布尔字段（缺省 false）。 */
    private static boolean boolField(String body, String key) {
        String needle = "\"" + key + "\"";
        int at = body.indexOf(needle);
        if (at < 0) {
            return false;
        }
        int colon = body.indexOf(':', at + needle.length());
        if (colon < 0) {
            return false;
        }
        String rest = body.substring(colon + 1, Math.min(body.length(), colon + 12)).trim();
        return rest.startsWith("true");
    }

    // -----------------------------------------------------------------------
    // 写出辅助
    // -----------------------------------------------------------------------

    private static String quote(String text) {
        return "\"" + escape(text) + "\"";
    }

    private static String stringArray(List<String> values) {
        return values.stream().map(JsoupRuleOracle::quote).collect(Collectors.joining(", ", "[", "]"));
    }

    private static String escape(String text) {
        if (text == null) {
            return "";
        }
        StringBuilder out = new StringBuilder();
        for (int i = 0; i < text.length(); i++) {
            char c = text.charAt(i);
            switch (c) {
                case '"':  out.append("\\\""); break;
                case '\\': out.append("\\\\"); break;
                case '\n': out.append("\\n"); break;
                case '\r': out.append("\\r"); break;
                case '\t': out.append("\\t"); break;
                default:
                    if (c < 0x20) {
                        out.append(String.format("\\u%04x", (int) c));
                    } else {
                        out.append(c);
                    }
            }
        }
        return out.toString();
    }

    /** 求值结果。 */
    private static final class Result {
        final List<String> values;
        final String error;

        private Result(List<String> values, String error) {
            this.values = values;
            this.error = error;
        }

        static Result success(List<String> values) {
            return new Result(values, null);
        }

        static Result failure(String error) {
            return new Result(Arrays.asList(), error);
        }
    }

    private JsoupRuleOracle() {
        // 工具类
    }
}
