package demo;

import java.util.List;

public class Main {
    /** Javadoc：中文說明 */
    public static void main(String[] args) {
        List<String> names = List.of("甲", "乙", "丙");
        names.forEach(n -> System.out.println("Hello, " + n));
    }
}
