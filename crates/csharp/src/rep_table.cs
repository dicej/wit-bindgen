/**
 * This class is used to assign a unique integer identifier to instances of
 * exported resources, which the host will use as its "core representation" per
 * https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#definition-types.
 * The identifier may be used to retrieve the corresponding instance e.g. when
 * lifting a handle as part of the canonical ABI implementation.
 */
internal class RepTable {
    private static List<object> list = new List<object>();
    private static int? firstVacant = null;
    
    private class Vacant {
        internal int? next;

        internal Vacant(int? next) {
            this.next = next;
        }
    }

    internal static int Add(object v) {
        int rep;
        if (firstVacant.HasValue) {
            rep = (int) firstVacant;
            firstVacant = ((Vacant) list[rep]).next;
            list[rep] = v;
        } else {
            rep = list.Count();
            list.Add(v);
        }
        return rep;
    }

    internal static object Get(int rep) {
        if (list[rep] is Vacant) {
            throw new ArgumentException("invalid rep");
        }
        return list[rep];
    }

    internal static object Remove(int rep) {
        var val = Get(rep);
        list[rep] = new Vacant(firstVacant);
        firstVacant = rep;
        return val;
    }
}
